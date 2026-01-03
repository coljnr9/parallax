use tokio::sync::{mpsc, oneshot};
use crate::types::{UpstreamHealth, Result};
use crate::hardening::{CircuitBreaker, CircuitState};
use crate::tui::TuiEvent;
use tokio::sync::broadcast;

pub enum KernelCommand {
    UpdateHealth { success: bool },
    CheckCircuit { resp: oneshot::Sender<Result<()>> },
    RecordCircuitSuccess,
    RecordCircuitFailure,
    GetHealth { resp: oneshot::Sender<HealthSnapshot> },
}

#[derive(Debug, Clone)]
pub struct HealthSnapshot {
    pub consecutive_failures: u32,
    pub total_requests: u32,
    pub failed_requests: u32,
    pub circuit_state: CircuitState,
}

pub struct Kernel {
    health: UpstreamHealth,
    circuit_breaker: CircuitBreaker,
    tx_tui: broadcast::Sender<TuiEvent>,
    rx_cmd: mpsc::Receiver<KernelCommand>,
}

impl Kernel {
    pub fn new(
        circuit_threshold: u32,
        recovery_timeout: std::time::Duration,
        tx_tui: broadcast::Sender<TuiEvent>,
        rx_cmd: mpsc::Receiver<KernelCommand>,
    ) -> Self {
        Self {
            health: UpstreamHealth::default(),
            circuit_breaker: CircuitBreaker::new(circuit_threshold, recovery_timeout),
            tx_tui,
            rx_cmd,
        }
    }

    pub async fn run(mut self) {
        tracing::info!("Kernel event loop started");
        while let Some(cmd) = self.rx_cmd.recv().await {
            match cmd {
                KernelCommand::UpdateHealth { success } => {
                    if success {
                        self.health.record_success();
                    } else {
                        self.health.record_failure();
                    }
                    self.emit_health_update().await;
                }
                KernelCommand::CheckCircuit { resp } => {
                    let _ = resp.send(self.circuit_breaker.check().await);
                }
                KernelCommand::RecordCircuitSuccess => {
                    self.circuit_breaker.record_success().await;
                    self.emit_health_update().await;
                }
                KernelCommand::RecordCircuitFailure => {
                    self.circuit_breaker.record_failure().await;
                    self.emit_health_update().await;
                }
                KernelCommand::GetHealth { resp } => {
                    let snapshot = HealthSnapshot {
                        consecutive_failures: self.health.consecutive_failures.load(std::sync::atomic::Ordering::Relaxed),
                        total_requests: self.health.total_requests.load(std::sync::atomic::Ordering::Relaxed) as u32,
                        failed_requests: self.health.failed_requests.load(std::sync::atomic::Ordering::Relaxed) as u32,
                        circuit_state: *self.circuit_breaker.state_raw_lock().await,
                    };
                    let _ = resp.send(snapshot);
                }
            }
        }
    }

    async fn emit_health_update(&self) {
        let _ = self.tx_tui.send(TuiEvent::UpstreamHealthUpdate {
            consecutive_failures: self.health.consecutive_failures.load(std::sync::atomic::Ordering::Relaxed),
            total_requests: self.health.total_requests.load(std::sync::atomic::Ordering::Relaxed),
            failed_requests: self.health.failed_requests.load(std::sync::atomic::Ordering::Relaxed),
            degraded: self.health.consecutive_failures.load(std::sync::atomic::Ordering::Relaxed) > 0,
        });
    }
}

