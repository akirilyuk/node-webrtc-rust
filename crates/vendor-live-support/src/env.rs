//! Environment handling for live tests: a missing key skips the test locally and fails it in CI.

use std::collections::HashMap;

/// Reads `vars` for `vendor_id`'s live test.
///
/// - All set and non-empty: `Some(map)`.
/// - Any missing or empty and `<VENDOR_ID_UPPER>_LIVE_REQUIRED == "1"`: panics naming every
///   missing variable (never a value), so CI cannot go green on a skipped run.
/// - Otherwise prints `<vendor_id> live test skipped: <VARS> unset` and returns `None`.
pub fn live_env(vendor_id: &str, vars: &[&str]) -> Option<HashMap<String, String>> {
    match live_env_from(vendor_id, vars, |name| std::env::var(name).ok()) {
        Ok(Some(map)) => Some(map),
        Ok(None) => {
            println!(
                "{vendor_id} live test skipped: {} unset",
                missing_list(vars, |name| std::env::var(name).ok())
            );
            None
        }
        Err(message) => panic!("{message}"),
    }
}

/// An optional variable: `None` when unset or empty.
pub fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn required_var(vendor_id: &str) -> String {
    format!("{}_LIVE_REQUIRED", vendor_id.to_uppercase())
}

fn missing_list(vars: &[&str], lookup: impl Fn(&str) -> Option<String>) -> String {
    vars.iter()
        .filter(|name| lookup(name).filter(|v| !v.is_empty()).is_none())
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

/// Testable core of [`live_env`]: `Ok(None)` = skip, `Err(message)` = must panic.
pub fn live_env_from(
    vendor_id: &str,
    vars: &[&str],
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Option<HashMap<String, String>>, String> {
    let mut found = HashMap::new();
    let mut missing = Vec::new();
    for name in vars {
        match lookup(name).filter(|value| !value.is_empty()) {
            Some(value) => {
                found.insert((*name).to_string(), value);
            }
            None => missing.push(*name),
        }
    }
    if missing.is_empty() {
        return Ok(Some(found));
    }
    let flag = required_var(vendor_id);
    if lookup(&flag).as_deref() == Some("1") {
        return Err(format!(
            "{flag}=1 but these variables are empty: {}",
            missing.join(", ")
        ));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn all_set_returns_the_values() {
        let env = [("A_KEY", "k1"), ("A_REGION", "r1")];
        let got = live_env_from("a", &["A_KEY", "A_REGION"], lookup(&env))
            .unwrap()
            .unwrap();
        assert_eq!(got["A_KEY"], "k1");
        assert_eq!(got["A_REGION"], "r1");
    }

    #[test]
    fn missing_skips_without_the_required_flag() {
        let env = [("A_KEY", "k1")];
        assert!(live_env_from("a", &["A_KEY", "A_REGION"], lookup(&env))
            .unwrap()
            .is_none());
        // Empty counts as missing.
        let env = [("A_KEY", "")];
        assert!(live_env_from("a", &["A_KEY"], lookup(&env))
            .unwrap()
            .is_none());
    }

    #[test]
    fn missing_with_the_required_flag_names_every_missing_var_but_no_value() {
        let env = [("A_LIVE_REQUIRED", "1"), ("A_KEY", "secret-value")];
        let err = live_env_from("a", &["A_KEY", "A_REGION", "A_VOICE"], lookup(&env)).unwrap_err();
        assert!(err.contains("A_REGION") && err.contains("A_VOICE"), "{err}");
        assert!(err.contains("A_LIVE_REQUIRED"), "{err}");
        assert!(!err.contains("secret-value"), "{err}");
        assert!(!err.contains("A_KEY"), "{err}");
    }

    #[test]
    fn required_flag_other_than_one_still_skips() {
        let env = [("A_LIVE_REQUIRED", "0")];
        assert!(live_env_from("a", &["A_KEY"], lookup(&env))
            .unwrap()
            .is_none());
    }
}
