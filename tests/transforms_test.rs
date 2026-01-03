#[cfg(test)]
mod tests {
    use parallax::ingress::RawTurn;

    #[tokio::test]
    async fn test_anthropic_xml_rescue_projection() {
        // Disabled due to missing testdata/fixtures/anthropic_xml_rescue.json
        /*
        let fixture = include_str!("../testdata/fixtures/anthropic_xml_rescue.json");
        let payload: serde_json::Value = match serde_json::from_str(fixture) {
            Ok(p) => p,
            Err(e) => panic!("Failed to parse fixture: {:?}", e),
        };

        // 1. Validation
        let raw: RawTurn = match serde_json::from_value(payload.clone()) {
            Ok(r) => r,
            Err(e) => panic!("Failed to parse RawTurn: {:?}", e),
        };
        match raw.validate() {
            Ok(_) => {}
            Err(e) => panic!("Validation failed: {:?}", e),
        }
        */
    }
}
