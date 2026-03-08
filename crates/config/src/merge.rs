use serde_yml::Value;

/// Apply an RFC 7396-style merge patch to a base YAML value.
pub fn apply_merge_patch(base: &Value, patch: &Value) -> Value {
    let Some(patch_map) = patch.as_mapping() else {
        return patch.clone();
    };

    let mut result = match base.as_mapping() {
        Some(m) => m.clone(),
        None => serde_yml::Mapping::new(),
    };

    for (key, value) in patch_map {
        if value.is_null() {
            result.remove(key);
        } else if value.as_mapping().is_some() {
            let base_val = result.get(key).unwrap_or(&Value::Null);
            let merged = apply_merge_patch(base_val, value);
            result.insert(key.clone(), merged);
        } else {
            result.insert(key.clone(), value.clone());
        }
    }

    Value::Mapping(result)
}

/// Compare two YAML values and return dot-separated paths of all differences.
pub fn diff_paths(old: &Value, new: &Value, prefix: &str) -> Vec<String> {
    if let (Some(old_map), Some(new_map)) = (old.as_mapping(), new.as_mapping()) {
        let mut paths = Vec::new();

        // Collect all keys from both maps
        let mut all_keys = Vec::new();
        for (k, _) in old_map {
            all_keys.push(k.clone());
        }
        for (k, _) in new_map {
            if !old_map.contains_key(k) {
                all_keys.push(k.clone());
            }
        }

        for key in &all_keys {
            let key_str = match key.as_str() {
                Some(s) => s.to_owned(),
                None => format!("{key:?}"),
            };
            let path = if prefix.is_empty() {
                key_str
            } else {
                format!("{prefix}.{key_str}")
            };

            match (old_map.get(key), new_map.get(key)) {
                (Some(_), None) => paths.push(path),
                (None, Some(_)) => paths.push(path),
                (Some(ov), Some(nv)) => {
                    paths.extend(diff_paths(ov, nv, &path));
                }
                (None, None) => unreachable!(),
            }
        }

        paths
    } else if old == new {
        Vec::new()
    } else if prefix.is_empty() {
        vec![String::new()]
    } else {
        vec![prefix.to_owned()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(s: &str) -> Value {
        serde_yml::from_str(s).unwrap()
    }

    #[test]
    fn test_merge_overwrites_scalar() {
        let base = yaml("a: 1");
        let patch = yaml("a: 2");
        let result = apply_merge_patch(&base, &patch);
        assert_eq!(result, yaml("a: 2"));
    }

    #[test]
    fn test_merge_recursive_objects() {
        let base = yaml(
            "gateway:\n  host: localhost\n  port: 8080\ndb:\n  url: postgres://localhost",
        );
        let patch = yaml("gateway:\n  port: 9090\n  tls: true");
        let result = apply_merge_patch(&base, &patch);
        let expected = yaml(
            "gateway:\n  host: localhost\n  port: 9090\n  tls: true\ndb:\n  url: postgres://localhost",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn test_merge_null_removes_key() {
        let base = yaml("a: 1\nb: 2\nc: 3");
        let patch = yaml("b: null");
        let result = apply_merge_patch(&base, &patch);
        let expected = yaml("a: 1\nc: 3");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_merge_non_object_patch_replaces() {
        let base = yaml("a: 1\nb: 2");
        let patch_str = yaml("\"replaced\"");
        assert_eq!(apply_merge_patch(&base, &patch_str), patch_str);

        let patch_seq: Value = serde_yml::from_str("[1, 2, 3]").unwrap();
        assert_eq!(apply_merge_patch(&base, &patch_seq), patch_seq);
    }

    #[test]
    fn test_diff_paths_detects_changes() {
        let old = yaml("a: 1\nb: 2");
        let new = yaml("a: 1\nb: 3");
        let paths = diff_paths(&old, &new, "");
        assert_eq!(paths, vec!["b"]);
    }

    #[test]
    fn test_diff_paths_nested() {
        let old = yaml("gateway:\n  host: localhost\n  port: 8080");
        let new = yaml("gateway:\n  host: localhost\n  port: 9090");
        let paths = diff_paths(&old, &new, "");
        assert_eq!(paths, vec!["gateway.port"]);
    }

    #[test]
    fn test_diff_paths_no_changes() {
        let val = yaml("a: 1\nb: 2");
        let paths = diff_paths(&val, &val, "");
        assert!(paths.is_empty());
    }
}
