use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const AI_STREAM_DECK_MODEL: &str = "AI Stream Deck";
const ROTATE_PLUGIN: &str = "com.elgato.streamdeck.profile.rotate";
const KEYPAD: &str = "Keypad";
const AI_COLS: i64 = 8;
const AI_ROWS: i64 = 4;

pub fn get_profiles_dir() -> Result<PathBuf> {
    let appdata =
        std::env::var("APPDATA").map_err(|_| anyhow!("APPDATA is not set; need Windows user profile"))?;
    Ok(PathBuf::from(appdata)
        .join("Elgato")
        .join("StreamDeck")
        .join("ProfilesV3"))
}

#[derive(Debug, Clone)]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub device_model: String,
    pub device_id: String,
}

pub fn get_profiles() -> Result<Vec<ProfileSummary>> {
    let dir = &get_profiles_dir()?;
    if !dir.is_dir() {
        bail!("Profiles directory does not exist: {}", dir.display());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sdProfile") {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest: Value = read_json(&manifest_path)?;
        let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let name = manifest
            .get("Name")
            .and_then(|n| n.as_str())
            .unwrap_or(&id)
            .to_string();
        let device_model = manifest
            .pointer("/Device/Model")
            .and_then(|m| m.as_str())
            .unwrap_or("(unknown)")
            .to_string();
        let device_id = manifest
            .pointer("/Device/UUID")
            .and_then(|m| m.as_str())
            .unwrap_or("(unknown)")
            .to_string();
        out.push(ProfileSummary {
            id,
            name,
            device_model,
            device_id
        });
    }
    out.sort_by_key(|p| format!("{}{}", p.device_model, p.name));
    Ok(out)
}

pub fn read_json(path: &Path) -> Result<Value> {
    let s = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&s).with_context(|| format!("parse JSON {}", path.display()))
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_string(value)?;
    fs::write(&tmp, data).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Returns (profile directory, root manifest JSON).
pub fn find_ai_stream_deck_profile(root: &Path) -> Result<(PathBuf, Value)> {
    let mut hits = Vec::new();
    for p in fs::read_dir(root).with_context(|| format!("read_dir {}", root.display()))? {
        let p = p?.path();
        if p.extension().and_then(|e| e.to_str()) != Some("sdProfile") {
            continue;
        }
        let manifest_path = p.join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest_json = read_json(&manifest_path)?;
        let model = manifest_json
            .pointer("/Device/Model")
            .and_then(|m| m.as_str())
            .unwrap_or("");
        if model == AI_STREAM_DECK_MODEL {
            hits.push((p, manifest_json));
        }
    }
    let n = hits.len();
    match n {
        0 => {
            bail!(
                "No profile with Device.Model == \"{AI_STREAM_DECK_MODEL}\".",
            );
        }
        1 => Ok(hits.into_iter().next().unwrap()),
        _ => bail!(
            "Multiple profiles match Device.Model == \"{AI_STREAM_DECK_MODEL}\" ({n} hits); expected exactly one."
        ),
    }
}

fn page_dir_name_for_uuid(page_uuid: &str) -> String {
    page_uuid.to_uppercase()
}

pub fn current_page_manifest_path(profile_dir: &Path, manifest_json: &Value) -> Result<PathBuf> {
    let current = manifest_json
        .pointer("/Pages/Current")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("profile manifest missing Pages.Current"))?;
    let dir = page_dir_name_for_uuid(current);
    let path = profile_dir.join("Profiles").join(dir).join("manifest.json");
    if !path.is_file() {
        bail!(
            "Current page manifest not found: {} (Pages.Current={})",
            path.display(),
            current
        );
    }
    Ok(path)
}

fn default_page_index(manifest_json: &Value) -> i64 {
    let Some(page_data) = manifest_json.get("Pages") else {
        return 1;
    };
    let Some(page_list) = page_data.get("Pages").and_then(|p| p.as_array()) else {
        return 1;
    };
    let Some(def) = page_data.get("Default").and_then(|u| u.as_str()) else {
        return 1;
    };
    if let Some(ix) = page_list.iter().position(|u| u.as_str() == Some(def)) {
        return ix as i64 + 1;
    }
    1
}

fn get_actions_mut(page_manifest: &mut Value) -> Result<&mut Map<String, Value>> {
    let controllers = page_manifest
        .get_mut("Controllers")
        .and_then(|c| c.as_array_mut())
        .ok_or_else(|| anyhow!("page manifest missing Controllers array"))?;
    for controller in controllers.iter_mut() {
        if controller.get("Type").and_then(|t| t.as_str()) == Some(KEYPAD) {
            if let Some(obj) = controller.as_object_mut() {
                match obj.get("Actions") {
                    None | Some(Value::Null) => {
                        obj.insert("Actions".into(), json!({}));
                    }
                    _ => {}
                }
            }
            let actions = controller
                .get_mut("Actions")
                .and_then(|a| a.as_object_mut())
                .ok_or_else(|| anyhow!("Keypad Actions must be an object"))?;
            return Ok(actions);
        }
    }
    bail!("page manifest has no Keypad controller")
}

fn position_key(col: i64, row: i64) -> String {
    format!("{col},{row}")
}

fn parse_position(key: &str) -> Option<(i64, i64)> {
    let mut parts = key.split(',');
    let c = parts.next()?.parse().ok()?;
    let r = parts.next()?.parse().ok()?;
    Some((c, r))
}

fn occupied_keypad_slots(page_manifest: &Value) -> Result<std::collections::HashSet<(i64, i64)>> {
    use std::collections::HashSet;
    let mut used = HashSet::new();
    let controllers = page_manifest
        .get("Controllers")
        .and_then(|c| c.as_array())
        .ok_or_else(|| anyhow!("page manifest missing Controllers array"))?;
    let mut found_keypad = false;
    for c in controllers {
        if c.get("Type").and_then(|t| t.as_str()) != Some(KEYPAD) {
            continue;
        }
        found_keypad = true;
        let Some(actions_val) = c.get("Actions") else {
            continue;
        };
        if actions_val.is_null() {
            continue;
        }
        let Some(actions) = actions_val.as_object() else {
            continue;
        };
        for (k, v) in actions {
            if v.is_null() || is_empty_action_slot(v) {
                continue;
            }
            if let Some((col, row)) = parse_position(k) {
                used.insert((col, row));
            }
        }
    }
    if !found_keypad {
        bail!("page manifest has no Keypad controller");
    }
    Ok(used)
}

/// Free slots (col,row) for AI Stream Deck keypad on this page (row-major order).
pub fn empty_keypad_slots(page_manifest: &Value) -> Result<Vec<(i64, i64)>> {
    let used = occupied_keypad_slots(page_manifest)?;
    let mut free = Vec::new();
    for row in 0..AI_ROWS {
        for col in 0..AI_COLS {
            if !used.contains(&(col, row)) {
                free.push((col, row));
            }
        }
    }
    Ok(free)
}

fn is_empty_action_slot(v: &Value) -> bool {
    v.as_object().map(|o| o.is_empty()).unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct Action {
    pub id: String,
    pub name: String,
    pub title: String,
}

/// List every action on the AI profile across all pages (for `list-actions`).
pub fn iter_ai_profile_actions(profile_dir: &Path) -> Result<Vec<Action>> {
    let mut out = Vec::new();
    let profiles_dir = profile_dir.join("Profiles");
    if !profiles_dir.is_dir() {
        return Ok(out);
    }
    let mut page_dirs: Vec<_> = fs::read_dir(&profiles_dir)
        .with_context(|| format!("read {}", profiles_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("manifest.json").is_file())
        .collect();
    page_dirs.sort();
    for page_dir in page_dirs {
        let man_path = page_dir.join("manifest.json");
        let page_manifest = read_json(&man_path)?;
        let Some(controllers) = page_manifest.get("Controllers").and_then(|c| c.as_array()) else {
            continue;
        };
        for controller in controllers {
            let Some(actions) = controller.get("Actions").and_then(|a| a.as_object()) else {
                continue;
            };
            for action in actions.values().cloned() {
                let action_id = action
                    .get("ActionID")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let name = action
                    .get("Name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let title = action.get("States").and_then(|c| c.as_array())
                    .and_then(|states| states.iter().next())
                    .and_then(|first_state| first_state.get("Title"))
                    .and_then(|title| title.as_str())
                    .unwrap_or("");

                out.push(Action {
                    id: action_id,
                    name,
                    title: title.into(),
                });
            }
        }
    }
    Ok(out)
}

/// Add switch-to-profile actions to the AI profile
pub fn add_profile_switch_actions(
    profiles_dir: &Path,
    ai_profile_dir: &Path,
    ai_profile_manifest: &Value,
) -> Result<(usize, Vec<String>, Vec<String>)> {
    let ai_id = ai_profile_dir
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("invalid profile dir"))?
        .to_string();

    let page_path = current_page_manifest_path(ai_profile_dir, ai_profile_manifest)?;
    let mut page_manifest = read_json(&page_path)?;

    let template_base = json!({
        "LinkedTitle": true,
        "Name": "Switch Profile",
        "Plugin": {
            "Name": "Switch Profile",
            "UUID": ROTATE_PLUGIN,
            "Version": "1.0"
        },
        "Resources": null,
        "Settings": {},
        "State": 0,
        "States": [{}],
        "UUID": ROTATE_PLUGIN
    });

    let mut free_slots = empty_keypad_slots(&page_manifest)?;
    let summaries = get_profiles()?;
    let target_profiles: Vec<_> = summaries
        .into_iter()
        .filter(|s| s.id.to_lowercase() != ai_id.to_lowercase())
        .collect();

    let actions = get_actions_mut(&mut page_manifest)?;
    let mut added = 0usize;
    let mut skipped = Vec::new();
    let mut reasons = Vec::new();

    for target_profile in target_profiles {
        let target_dir = profiles_dir.join(format!("{}.sdProfile", target_profile.id));
        let target_manifest_path = target_dir.join("manifest.json");
        if !target_manifest_path.is_file() {
            skipped.push(target_profile.id.clone());
            reasons.push("missing manifest".into());
            continue;
        }
        let Some((col, row)) = free_slots.first().copied() else {
            skipped.push(target_profile.id.clone());
            reasons.push("no empty keypad slots".into());
            continue;
        };
        free_slots.remove(0);

        let target_profile_manifest_json = read_json(&target_manifest_path)?;
        let default_page_index = default_page_index(&target_profile_manifest_json);

        let mut action = template_base.clone();
        if let Some(obj) = action.as_object_mut() {
            obj.insert(
                "ActionID".into(),
                json!(uuid::Uuid::new_v4().to_string()),
            );
            let settings = obj
                .entry("Settings")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| anyhow!("Settings must be object"))?;
            settings.insert("DeviceUUID".into(), json!(target_profile.device_id));
            settings.insert("PageIndex".into(), json!(default_page_index));
            settings.insert("ProfileUUID".into(), json!(target_profile.id));
            if let Some(states) = obj.get_mut("States").and_then(|s| s.as_array_mut())
                && let Some(first) = states.first_mut().and_then(|x| x.as_object_mut())
            {
                first.insert("Title".into(), json!(target_profile.name));
            }
        }

        let key = position_key(col, row);
        actions.insert(key, action);
        added += 1;
    }

    write_json_atomic(&page_path, &page_manifest)?;
    Ok((added, skipped, reasons))
}
