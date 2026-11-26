//! Opt-in exact integer projection at the original public response boundary.
//! Returned data only: no native archive, ticket, record, command or grant.
use serde_json::{Value, json};
use std::io::{self, Write};

pub(super) const ENCODING: &str = "native-integer-decimal-v1";
pub(super) const MAX_REPLY_BYTES: usize = 24 * 1024 * 1024;

fn supported(method: &str, params: &Value) -> bool {
    matches!(method, "engine.execute" | "ui.menu.invoke")
        && matches!(
            params.get("command").or_else(|| params.get("id")).and_then(Value::as_str),
            Some(
                "agent.readMulticamInputs"
                    | "agent.snapshotStatus"
                    | "agent.cancelSnapshot"
                    | "agent.prepareMulticamProduction"
                    | "agent.readMulticamProductionPlan"
                    | "agent.apply"
                    | "agent.receipt"
                    | "agent.readHistory"
                    | "agent.undo"
                    | "agent.redo"
            )
        )
}

/// Remove transport negotiation before the original handler sees the request.
/// Invalid negotiation is refused BEFORE dispatch; never fall back to legacy numbers.
pub(super) fn take_request(method: &str, params: &mut Value) -> Result<bool, String> {
    let Some(encoding) = params.get("sourceIntegerEncoding") else { return Ok(false) };
    if encoding != ENCODING || !supported(method, params) {
        return Err("unsupported exact source integer encoding or command; nothing dispatched".into());
    }
    if params.get("command").or_else(|| params.get("id")).and_then(Value::as_str) == Some("agent.prepareMulticamProduction") {
        if let Some(offset) = params.get_mut("params").and_then(|v| v.get_mut("edit")).and_then(|v| v.get_mut("options")).and_then(|v| v.get_mut("offset")) {
            if let Value::String(text) = offset {
                if text.len() > 20 {
                    return Err("source offset must be a canonical signed-i64 decimal; nothing dispatched".into());
                }
                let number = text.parse::<i64>().map_err(|_| "source offset is outside signed-i64; nothing dispatched")?;
                if text != &number.to_string() {
                    return Err("source offset must be a canonical signed-i64 decimal; nothing dispatched".into());
                }
                *offset = json!(number);
            }
        }
    }
    let Some(params) = params.as_object_mut() else { return Err("control parameters must be an object".into()) };
    params.remove("sourceIntegerEncoding");
    Ok(true)
}

struct BoundedSize(usize);
impl Write for BoundedSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next =
            self.0.checked_add(bytes.len()).filter(|n| *n <= MAX_REPLY_BYTES).ok_or_else(|| io::Error::other("complete exact source reply exceeds 24 MiB"))?;
        self.0 = next;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bounded(value: &Value) -> bool {
    serde_json::to_writer(BoundedSize(0), value).is_ok()
}

/// Complete iterative conversion ONLY in the explicitly negotiated result.
/// All native i64/u64 values become canonical decimal strings, including small integers.
/// Strings/opaque archives, floats, nulls, arrays and keys retain their values.
/// Work-list length is bounded by the number of nodes in the bounded complete input.
pub(super) fn project(mut reply: Value) -> Value {
    let valid_envelope = reply.is_object() && reply.get("ok").and_then(Value::as_bool).is_some() && (reply["ok"] != true || reply.get("result").is_some());
    if !valid_envelope || !bounded(&reply) {
        return unavailable();
    }
    if let Some(result) = reply.get_mut("result") {
        let mut pending = vec![result];
        while let Some(value) = pending.pop() {
            match value {
                Value::Number(n) if n.is_i64() || n.is_u64() => *value = Value::String(n.to_string()),
                Value::Array(rows) => pending.extend(rows.iter_mut()),
                Value::Object(rows) => pending.extend(rows.values_mut()),
                _ => {}
            }
        }
    }
    if let Some(object) = reply.as_object_mut() {
        object.insert("sourceIntegerEncoding".into(), json!(ENCODING));
    }
    if bounded(&reply) { reply } else { unavailable() }
}
fn unavailable() -> Value {
    // Dispatch happened. Invalid/oversized output cannot prove rollback.
    json!({"ok":false,"outcome":"unknown","sourceIntegerEncoding":ENCODING,
        "error":"complete exact source reply unavailable; observe the original retained plan or receipt before another edit"})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_full_i64_offset_uses_original_parameter_without_rounding_other_strings() {
        for number in [i64::MIN, -9_007_199_254_740_993, -1, 0, 1, 9_007_199_254_740_993, i64::MAX] {
            let mut request = json!({"command":"agent.prepareMulticamProduction","sourceIntegerEncoding":ENCODING,
                "params":{"edit":{"gesture":"synchronize","options":{"offset":number.to_string(),"marker":"001"}}}});
            assert_eq!(take_request("engine.execute", &mut request), Ok(true));
            assert_eq!(request["params"]["edit"]["options"]["offset"].as_i64(), Some(number));
            assert_eq!(request["params"]["edit"]["options"]["marker"], "001");
        }
        for text in ["-0", "+1", "01", "1.0", "1e3", " 1", "9223372036854775808", "-9223372036854775809", ""] {
            let mut request = json!({"command":"agent.prepareMulticamProduction","sourceIntegerEncoding":ENCODING,
                "params":{"edit":{"options":{"offset":text}}}});
            assert!(take_request("engine.execute", &mut request).is_err(), "{text}");
        }
    }

    #[test]
    fn native_integer_limits_and_nested_fields_are_exact_without_parsing_archives() {
        let archive = "{\"tick\":9223372036854775807}";
        let raw = json!({"ok":true,"result":{"negative":i64::MIN,"positive":i64::MAX,"unsigned":u64::MAX,
            "nested":[0,-1,9_007_199_254_740_992i64,{"rate":{"num":24000,"den":1001},"automatic":null}],
            "floating":1.25,"wholeFloat":1.0,"text":"001","archive":archive}});
        let original = raw.clone();
        let wire = project(raw);
        assert_eq!(wire["result"]["negative"], "-9223372036854775808");
        assert_eq!(wire["result"]["positive"], "9223372036854775807");
        assert_eq!(wire["result"]["unsigned"], "18446744073709551615");
        assert_eq!(wire["result"]["nested"][2], "9007199254740992");
        assert_eq!(wire["result"]["nested"][3]["rate"]["den"], "1001");
        assert_eq!(wire["result"]["floating"], 1.25);
        assert!(wire["result"]["wholeFloat"].is_f64());
        assert_eq!(wire["result"]["archive"], archive);
        assert_eq!(wire["result"]["text"], "001");
        assert_eq!(original["result"]["positive"], i64::MAX);
    }
    #[test]
    fn finite_negotiation_is_removed_without_touching_native_parameters() {
        for command in [
            "agent.readMulticamInputs",
            "agent.snapshotStatus",
            "agent.cancelSnapshot",
            "agent.prepareMulticamProduction",
            "agent.readMulticamProductionPlan",
            "agent.apply",
            "agent.receipt",
            "agent.readHistory",
            "agent.undo",
            "agent.redo",
        ] {
            for method in ["engine.execute", "ui.menu.invoke"] {
                let key = if method == "engine.execute" { "command" } else { "id" };
                let mut request = json!({(key):command,"params":{"offset":i64::MIN},"sourceIntegerEncoding":ENCODING});
                assert_eq!(take_request(method, &mut request), Ok(true));
                assert!(request.get("sourceIntegerEncoding").is_none());
                assert_eq!(request["params"]["offset"], i64::MIN);
            }
        }
        for mut request in [
            json!({"command":"file.newProject","sourceIntegerEncoding":ENCODING}),
            json!({"command":"agent.apply","sourceIntegerEncoding":"future"}),
            json!({"command":"agent.apply","sourceIntegerEncoding":null}),
        ] {
            assert!(take_request("engine.execute", &mut request).is_err());
        }
        assert_eq!(take_request("engine.execute", &mut json!({"command":"agent.apply"})), Ok(false));
    }
    #[test]
    fn complete_large_results_and_postdispatch_failures_are_not_truncated_or_rejected() {
        let rows = (0..100_000).map(|i| json!({"time":i64::MAX,"index":i})).collect::<Vec<_>>();
        let result = project(json!({"ok":true,"result":{"instances":rows}}));
        assert_eq!(result["result"]["instances"].as_array().unwrap().len(), 100_000);
        assert_eq!(result["result"]["instances"][99_999]["time"], i64::MAX.to_string());
        let oversized = project(json!({"ok":true,"result":"x".repeat(MAX_REPLY_BYTES)}));
        assert_eq!(oversized["ok"], false);
        assert_eq!(oversized["outcome"], "unknown");
        assert!(oversized.get("result").is_none());
        assert_eq!(project(json!({"ok":true}))["outcome"], "unknown");
        let refused = project(json!({"ok":false,"error":"source changed","outcome":"rejected"}));
        assert_eq!(refused["outcome"], "rejected");
        let unknown = project(json!({"ok":false,"error":"worker reply lost","outcome":"unknown"}));
        assert_eq!(unknown["outcome"], "unknown");
    }
}
