use serde_json::{Map, Value};


pub fn session_delta(old: &Value, new: &Value) -> Option<Value> {
    match (old, new) {
        (Value::Object(old_map), Value::Object(new_map)) => {
            let delta = object_delta(old_map, new_map);
            if delta.is_empty() { None } else { Some(Value::Object(delta)) }
        }

        (Value::Number(o), Value::Number(n)) => numeric_diff(o, n),

        _ if old == new => None,

        _ => Some(serde_json::json!({ "old": old, "new": new })),
    }
}


fn object_delta(old: &Map<String, Value>, new: &Map<String, Value>) -> Map<String, Value> {
    let mut delta = Map::new();

    for (key, new_val) in new {
        match old.get(key) {
            Some(old_val) => {
                if let Some(d) = session_delta(old_val, new_val) {
                    delta.insert(key.clone(), d);
                }
            }
            None => {
                delta.insert(key.clone(), serde_json::json!({ "old": Value::Null, "new": new_val }));
            }
        }
    }

    for (key, old_val) in old {
        if !new.contains_key(key) {
            delta.insert(key.clone(), serde_json::json!({ "old": old_val, "new": Value::Null }));
        }
    }

    delta
}


fn numeric_diff(old: &serde_json::Number, new: &serde_json::Number) -> Option<Value> {
    if let (Some(o), Some(n)) = (old.as_i64(), new.as_i64()) {
        let diff = n - o;
        return if diff == 0 { None } else { Some(Value::Number(diff.into())) };
    }

    if let (Some(o), Some(n)) = (old.as_f64(), new.as_f64()) {
        let diff = n - o;
        return if diff == 0.0 { None } else { serde_json::Number::from_f64(diff).map(Value::Number) };
    }

    Some(serde_json::json!({ "old": Value::Number(old.clone()), "new": Value::Number(new.clone()) }))
}
