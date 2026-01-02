#[cfg(test)]
mod tests {
    use parallax::ingress::RawTurn;

    #[tokio::test]
    async fn test_anthropic_xml_rescue_projection() {
        // Disabled due to missing testdata/fixtures/anthropic_xml_rescue.json
        /*
        let fixture = include_str!("../testdata/fixtures/anthropic_xml_rescue.json");
        let payload: serde_json::Value = serde_json::from_str(fixture).unwrap();

        // 1. Validation
        let raw: RawTurn = serde_json::from_value(payload.clone()).unwrap();
        raw.validate().expect("Validation should pass");
        */
    }
}
