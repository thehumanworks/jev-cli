//! A real call to `TypeSafe`. Ignored by `cargo test`. It spends credits and sends the state below
//! to the API, so run it only when asked: `cargo test --test live -- --ignored`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::time::Duration;

use assert_cmd::Command;
use serde_json::Value;

#[test]
#[ignore = "calls the TypeSafe API and spends credits"]
fn live_noul_when_a_key_is_set() {
    let key = std::env::var("TYPESAFE_API_KEY").ok().filter(|value| !value.trim().is_empty());
    let Some(key) = key else {
        // The ignored test is skipped, not failed, when nobody exported a key.
        let _ = writeln!(std::io::stderr(), "live test skipped: TYPESAFE_API_KEY is not set");
        return;
    };

    let output = Command::cargo_bin("jev")
        .unwrap()
        .env_clear()
        .env("TYPESAFE_API_KEY", key)
        .args([
            "noul",
            "Is the sky described as blue?",
            "-s",
            "The sky is blue.",
            "--json",
            "--retries",
            "2",
            "--timeout",
            "60",
        ])
        .timeout(Duration::from_secs(120))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "status: {}, stderr: {stderr}", output.status);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["answers"]["q"]["type"], "noul");
    assert!(value["cost_usd"].is_number(), "{stdout}");
}
