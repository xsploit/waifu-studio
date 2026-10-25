//! Typed, bounded plain integration settings. These values are not credentials:
//! no secret storage, secret references, or credential resolution is provided.
//! Deserialize for shape, then validate at every host/persistence boundary.
use serde::{Deserialize, Deserializer, Serialize, de};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_SETTINGS: usize = 32;
pub const MAX_SETTING_ID_BYTES: usize = 64;
pub const MAX_LABEL_BYTES: usize = 128;
pub const MAX_DESCRIPTION_BYTES: usize = 1024;
pub const MAX_STRING_BYTES: usize = 1024;
pub const MAX_CHOICE_OPTIONS: usize = 32;
pub const MAX_OPTION_BYTES: usize = 128;
pub const MAX_SETTINGS_BYTES: usize = 16 * 1024;

pub type Settings = BTreeMap<String, SettingValue>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    Integer(i64),
    Text(String),
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SettingDefinition {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub kind: SettingKind,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SettingKind {
    Toggle {
        default: bool,
    },
    Integer {
        default: i64,
        min: i64,
        max: i64,
    },
    Text {
        default: String,
        max_length: usize,
    },
    Choice {
        default: String,
        options: Vec<String>,
    },
    /// Registered motion ID stored as text; empty selects no motion. This is
    /// never a filesystem path. Catalog availability and permission are checked
    /// at runtime, so temporary unavailability does not erase saved choices.
    Motion {
        default: String,
    },
    /// Avatar expression name stored as text; empty selects no expression.
    /// Availability is checked at runtime, independently of saved settings.
    Expression {
        default: String,
    },
    /// Character UUID stored as text; empty selects the current loaded target.
    /// Nonempty values must be canonical lowercase, hyphenated, nonnil UUIDs.
    /// Character availability is a UI/runtime concern, never a migration check.
    Character {
        default: String,
    },
}

// serde's derived flatten + deny_unknown_fields combination is unsupported.
// Read the map once (rejecting duplicate fields), remove common fields, then
// deserialize the remaining tagged kind with its strict unknown-field policy.
impl<'de> Deserialize<'de> for SettingDefinition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DefinitionVisitor;
        impl<'de> de::Visitor<'de> for DefinitionVisitor {
            type Value = SettingDefinition;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a strictly typed setting definition")
            }
            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut fields = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, serde_json::Value>()? {
                    if fields.insert(key.clone(), value).is_some() {
                        return Err(de::Error::custom(format!(
                            "Duplicate setting field '{key}'"
                        )));
                    }
                }
                let id = fields
                    .remove("id")
                    .ok_or_else(|| de::Error::missing_field("id"))?;
                let label = fields
                    .remove("label")
                    .ok_or_else(|| de::Error::missing_field("label"))?;
                let description = fields
                    .remove("description")
                    .unwrap_or_else(|| serde_json::Value::String(String::new()));
                Ok(SettingDefinition {
                    id: serde_json::from_value(id).map_err(de::Error::custom)?,
                    label: serde_json::from_value(label).map_err(de::Error::custom)?,
                    description: serde_json::from_value(description).map_err(de::Error::custom)?,
                    kind: serde_json::from_value(serde_json::Value::Object(fields))
                        .map_err(de::Error::custom)?,
                })
            }
        }
        deserializer.deserialize_map(DefinitionVisitor)
    }
}

/// Checks schema identity, labels, defaults, integer ranges and bounded choices.
/// Also ensures the complete default configuration fits the wire budget.
pub fn validate_schema(schema: &[SettingDefinition]) -> Result<(), String> {
    if schema.len() > MAX_SETTINGS {
        return Err(format!("Settings schema exceeds {MAX_SETTINGS} entries"));
    }
    let mut defaults = Settings::new();
    for definition in schema {
        validate_id(&definition.id)?;
        checked_string(&definition.label, "Setting label", MAX_LABEL_BYTES, true)?;
        checked_string(
            &definition.description,
            "Setting description",
            MAX_DESCRIPTION_BYTES,
            false,
        )?;
        if defaults.contains_key(&definition.id) {
            return Err(format!("Duplicate setting id '{}'", definition.id));
        }
        match &definition.kind {
            SettingKind::Toggle { .. } => (),
            SettingKind::Integer { default, min, max } => {
                if min > max || default < min || default > max {
                    return Err(format!(
                        "Setting '{}' has invalid integer bounds/default",
                        definition.id
                    ));
                }
            }
            SettingKind::Text {
                default,
                max_length,
            } => {
                if *max_length > MAX_STRING_BYTES {
                    return Err(format!(
                        "Setting '{}' max_length exceeds {MAX_STRING_BYTES} bytes",
                        definition.id
                    ));
                }
                checked_string(default, "Text default", *max_length, false)?;
            }
            SettingKind::Choice { default, options } => {
                if options.is_empty() || options.len() > MAX_CHOICE_OPTIONS {
                    return Err(format!(
                        "Setting '{}' requires 1..{MAX_CHOICE_OPTIONS} choices",
                        definition.id
                    ));
                }
                let mut unique = BTreeSet::new();
                for option in options {
                    checked_string(option, "Choice option", MAX_OPTION_BYTES, true)?;
                    if !unique.insert(option) {
                        return Err(format!("Setting '{}' has duplicate choices", definition.id));
                    }
                }
                if !options.contains(default) {
                    return Err(format!(
                        "Setting '{}' default is not a choice",
                        definition.id
                    ));
                }
            }
            SettingKind::Motion { default } => checked_motion_id(default)?,
            SettingKind::Expression { default } => checked_expression_name(default)?,
            SettingKind::Character { default } => checked_character_id(default)?,
        }
        defaults.insert(definition.id.clone(), default_value(&definition.kind));
    }
    validate_wire_values(&defaults)
}

/// Schema-less checks for a host handshake settings payload. The bound is compact
/// serialized JSON bytes, including escaping, keys and punctuation. Count, key
/// and value bounds are checked before serialization: public callers cannot
/// trigger a serialization allocation proportional to unchecked input strings.
pub fn validate_wire_values(values: &Settings) -> Result<(), String> {
    if values.len() > MAX_SETTINGS {
        return Err(format!("Settings exceed {MAX_SETTINGS} entries"));
    }
    for (id, value) in values {
        validate_id(id)?;
        if let SettingValue::Text(text) = value {
            checked_string(text, "Setting text", MAX_STRING_BYTES, false)?;
        }
    }
    if serde_json::to_vec(values).map_err(|e| e.to_string())?.len() > MAX_SETTINGS_BYTES {
        return Err(format!(
            "Settings exceed {MAX_SETTINGS_BYTES} serialized bytes"
        ));
    }
    Ok(())
}

/// Fill omitted values from defaults; reject unknown, mistyped or invalid values.
pub fn validate_values(
    schema: &[SettingDefinition],
    values: &Settings,
) -> Result<Settings, String> {
    validate_schema(schema)?;
    validate_wire_values(values)?;
    for id in values.keys() {
        if !schema.iter().any(|definition| &definition.id == id) {
            return Err(format!("Unknown setting '{id}'"));
        }
    }
    let mut result = Settings::new();
    for definition in schema {
        let value = values
            .get(&definition.id)
            .cloned()
            .unwrap_or_else(|| default_value(&definition.kind));
        if !compatible(&definition.kind, &value) {
            return Err(format!(
                "Setting '{}' has the wrong type or is outside its allowed values",
                definition.id
            ));
        }
        result.insert(definition.id.clone(), value);
    }
    validate_wire_values(&result)?;
    Ok(result)
}

/// Keep individually compatible old values, replace incompatible/missing values
/// with defaults, and drop removed IDs. Old data is deliberately not validated
/// as a whole: obsolete/malformed entries must not prevent a bounded migration.
/// The schema and resulting complete configuration must satisfy all limits.
pub fn migrate_values(schema: &[SettingDefinition], old: &Settings) -> Result<Settings, String> {
    validate_schema(schema)?;
    let mut result = Settings::new();
    for definition in schema {
        let value = old
            .get(&definition.id)
            .filter(|value| compatible(&definition.kind, value))
            .cloned()
            .unwrap_or_else(|| default_value(&definition.kind));
        result.insert(definition.id.clone(), value);
    }
    validate_wire_values(&result)?;
    Ok(result)
}

fn default_value(kind: &SettingKind) -> SettingValue {
    match kind {
        SettingKind::Toggle { default } => SettingValue::Bool(*default),
        SettingKind::Integer { default, .. } => SettingValue::Integer(*default),
        SettingKind::Text { default, .. }
        | SettingKind::Choice { default, .. }
        | SettingKind::Motion { default }
        | SettingKind::Expression { default }
        | SettingKind::Character { default } => SettingValue::Text(default.clone()),
    }
}

fn compatible(kind: &SettingKind, value: &SettingValue) -> bool {
    match (kind, value) {
        (SettingKind::Toggle { .. }, SettingValue::Bool(_)) => true,
        (SettingKind::Integer { min, max, .. }, SettingValue::Integer(value)) => {
            value >= min && value <= max
        }
        (SettingKind::Text { max_length, .. }, SettingValue::Text(value)) => {
            checked_string(value, "Setting text", *max_length, false).is_ok()
        }
        (SettingKind::Choice { options, .. }, SettingValue::Text(value)) => options.contains(value),
        (SettingKind::Motion { .. }, SettingValue::Text(value)) => checked_motion_id(value).is_ok(),
        (SettingKind::Expression { .. }, SettingValue::Text(value)) => {
            checked_expression_name(value).is_ok()
        }
        (SettingKind::Character { .. }, SettingValue::Text(value)) => {
            checked_character_id(value).is_ok()
        }
        _ => false,
    }
}

fn checked_motion_id(value: &str) -> Result<(), String> {
    checked_string(
        value,
        "Motion ID",
        crate::extension_protocol::MAX_REGISTERED_ID_BYTES,
        false,
    )?;
    if value.chars().any(char::is_whitespace) {
        return Err("Motion ID must be empty or contain no whitespace".into());
    }
    Ok(())
}

fn checked_expression_name(value: &str) -> Result<(), String> {
    checked_string(
        value,
        "Expression name",
        crate::extension_protocol::MAX_ID_BYTES,
        !value.is_empty(),
    )?;
    if crate::behavior::reserved_expression(value) {
        return Err("Expression name is reserved for host face controls".into());
    }
    Ok(())
}

fn checked_character_id(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Ok(());
    }
    const ERROR: &str = "Character must be empty or a canonical lowercase nonnil hyphenated UUID";
    // Bound before parsing/formatting, including when migrating unchecked old
    // settings. Do not echo caller-controlled input into diagnostics.
    if value.len() != 36 {
        return Err(ERROR.into());
    }
    let id = uuid::Uuid::try_parse(value).map_err(|_| ERROR.to_string())?;
    if id.is_nil() || id.hyphenated().to_string() != value {
        return Err(ERROR.into());
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > MAX_SETTING_ID_BYTES
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        // Do not copy an unchecked caller-owned key into the diagnostic.
        return Err("Invalid setting id: expected a 1..64 byte lowercase slug".into());
    }
    Ok(())
}

fn checked_string(value: &str, label: &str, max: usize, nonempty: bool) -> Result<(), String> {
    if value.len() > max
        || value.chars().any(char::is_control)
        || (nonempty && value.trim().is_empty())
    {
        return Err(format!(
            "{label} must contain no control characters, fit {max} UTF-8 bytes{}",
            if nonempty { ", and be nonempty" } else { "" }
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn definition(id: &str, kind: SettingKind) -> SettingDefinition {
        SettingDefinition {
            id: id.into(),
            label: "Setting".into(),
            description: String::new(),
            kind,
        }
    }
    fn schema() -> Vec<SettingDefinition> {
        vec![
            definition("enabled", SettingKind::Toggle { default: true }),
            definition(
                "count",
                SettingKind::Integer {
                    default: 3,
                    min: -2,
                    max: 10,
                },
            ),
            definition(
                "title",
                SettingKind::Text {
                    default: "hello".into(),
                    max_length: 8,
                },
            ),
            definition(
                "mode",
                SettingKind::Choice {
                    default: "quiet".into(),
                    options: vec!["quiet".into(), "normal".into()],
                },
            ),
        ]
    }

    #[test]
    fn character_schema_is_strict_and_defaults_are_plain_text_on_wire() {
        for default in ["", "12345678-9abc-4def-8123-456789abcdef"] {
            let raw = json!({"id":"character", "label":"Character", "type":"character", "default":default});
            let parsed: SettingDefinition = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(
                parsed.kind,
                SettingKind::Character {
                    default: default.into()
                }
            );
            assert_eq!(serde_json::to_value(&parsed).unwrap()["type"], "character");
            assert_eq!(
                serde_json::from_value::<SettingDefinition>(serde_json::to_value(&parsed).unwrap())
                    .unwrap(),
                parsed
            );
            let defaults = validate_values(&[parsed], &Settings::new()).unwrap();
            assert_eq!(defaults["character"], SettingValue::Text(default.into()));
            assert_eq!(
                serde_json::to_value(&defaults).unwrap(),
                json!({"character":default})
            );

            for field in ["unknown", "min", "max", "max_length", "options"] {
                let mut invalid = raw.clone();
                invalid[field] = json!(null);
                assert!(serde_json::from_value::<SettingDefinition>(invalid).is_err());
            }
            for value in [json!(null), json!(true), json!(1), json!([]), json!({})] {
                let mut invalid = raw.clone();
                invalid["default"] = value;
                assert!(serde_json::from_value::<SettingDefinition>(invalid).is_err());
            }
            let mut missing = raw;
            missing.as_object_mut().unwrap().remove("default");
            assert!(serde_json::from_value::<SettingDefinition>(missing).is_err());
        }
        for invalid in [
            r#"{"id":"character","label":"Character","type":"character","default":"","default":"12345678-9abc-4def-8123-456789abcdef"}"#,
            r#"{"type":"character","default":"","unknown":true}"#,
            r#"{"type":"character","default":"","default":""}"#,
        ] {
            assert!(serde_json::from_str::<SettingKind>(invalid).is_err());
        }
        assert!(serde_json::from_str::<SettingDefinition>(
            r#"{"id":"character","label":"Character","type":"character","default":"","default":""}"#
        ).is_err());
    }

    #[test]
    fn character_defaults_and_values_reject_nil_noncanonical_malformed_and_wrong_types() {
        let schema = [definition(
            "character",
            SettingKind::Character {
                default: String::new(),
            },
        )];
        for invalid in [
            "00000000-0000-0000-0000-000000000000".to_owned(),
            "12345678-9ABC-4DEF-8123-456789ABCDEF".into(),
            "123456789abc4def8123456789abcdef".into(),
            "urn:uuid:12345678-9abc-4def-8123-456789abcdef".into(),
            "{12345678-9abc-4def-8123-456789abcdef}".into(),
            "12345678-9abc-4def-8123-456789abcdeg".into(),
            " 12345678-9abc-4def-8123-456789abcdef".into(),
            "12345678-9abc-4def-8123-456789abcdef ".into(),
            "12345678-9abc-4def-8123-456789abcdef\n".into(),
            "12345678-9abc-4def-8123-456789abcde\0".into(),
            "雪".repeat(12),
            " ".into(),
            "not-a-uuid".into(),
            "x".repeat(1024 * 1024),
        ] {
            let bad_schema = [definition(
                "character",
                SettingKind::Character {
                    default: invalid.clone(),
                },
            )];
            let error = validate_schema(&bad_schema).unwrap_err();
            assert!(error.len() < 128);
            assert!(migrate_values(&bad_schema, &Settings::new()).is_err());
            let values = Settings::from([("character".into(), SettingValue::Text(invalid))]);
            assert!(validate_values(&schema, &values).is_err());
            assert_eq!(
                migrate_values(&schema, &values).unwrap()["character"],
                SettingValue::Text(String::new())
            );
        }
        for value in [SettingValue::Bool(true), SettingValue::Integer(7)] {
            let values = Settings::from([("character".into(), value)]);
            assert!(validate_values(&schema, &values).is_err());
            assert_eq!(
                migrate_values(&schema, &values).unwrap()["character"],
                SettingValue::Text(String::new())
            );
        }
    }

    #[test]
    fn character_migration_preserves_unavailable_ids_and_current_target_resets_invalid_values() {
        let fallback = "12345678-9abc-4def-8123-456789abcdef";
        let schema = [definition(
            "character",
            SettingKind::Character {
                default: fallback.into(),
            },
        )];
        // No catalog or loaded avatar is consulted: even an unavailable UUID
        // remains configured, and empty remains the current-target selection.
        for selected in ["", "fedcba98-7654-4321-8abc-def012345678"] {
            let old = Settings::from([
                ("character".into(), SettingValue::Text(selected.into())),
                (
                    "removed".into(),
                    SettingValue::Text("x".repeat(MAX_STRING_BYTES + 1)),
                ),
            ]);
            let expected =
                Settings::from([("character".into(), SettingValue::Text(selected.into()))]);
            assert_eq!(migrate_values(&schema, &old).unwrap(), expected);
            assert_eq!(validate_values(&schema, &expected).unwrap(), expected);
        }
        let defaults = validate_values(&schema, &Settings::new()).unwrap();
        assert_eq!(defaults["character"], SettingValue::Text(fallback.into()));
        assert_eq!(migrate_values(&schema, &Settings::new()).unwrap(), defaults);
        for value in [
            SettingValue::Bool(false),
            SettingValue::Text("00000000-0000-0000-0000-000000000000".into()),
        ] {
            let old = Settings::from([("character".into(), value)]);
            assert_eq!(migrate_values(&schema, &old).unwrap(), defaults);
        }
    }

    #[test]
    fn motion_schema_is_strict_and_defaults_use_plain_text_on_wire() {
        for default in ["", "org.example.disabled/nod"] {
            let raw = json!({
                "id": "reaction", "label": "Reaction", "type": "motion", "default": default,
            });
            let definition: SettingDefinition = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(
                definition.kind,
                SettingKind::Motion {
                    default: default.into()
                }
            );
            let serialized = serde_json::to_value(&definition).unwrap();
            assert_eq!(serialized["type"], "motion");
            assert_eq!(serialized["default"], default);
            assert_eq!(
                serde_json::from_value::<SettingDefinition>(serialized).unwrap(),
                definition
            );
            let defaults = validate_values(&[definition], &Settings::new()).unwrap();
            assert_eq!(defaults["reaction"], SettingValue::Text(default.into()));
            assert_eq!(
                serde_json::to_value(&defaults).unwrap(),
                json!({"reaction": default})
            );
            validate_wire_values(&defaults).unwrap();

            for field in ["unknown", "min", "max_length", "options"] {
                let mut invalid = raw.clone();
                invalid[field] = json!(null);
                assert!(serde_json::from_value::<SettingDefinition>(invalid).is_err());
            }
            for invalid_default in [json!(null), json!(false), json!(1), json!([])] {
                let mut invalid = raw.clone();
                invalid["default"] = invalid_default;
                assert!(serde_json::from_value::<SettingDefinition>(invalid).is_err());
            }
            let mut missing = raw;
            missing.as_object_mut().unwrap().remove("default");
            assert!(serde_json::from_value::<SettingDefinition>(missing).is_err());
        }
        assert!(serde_json::from_str::<SettingDefinition>(
            r#"{"id":"reaction","label":"Reaction","type":"motion","default":"","default":"nod"}"#
        ).is_err());
    }

    #[test]
    fn motion_ids_match_action_limits_and_reject_all_whitespace_and_controls() {
        use crate::extension_protocol::{Action, MAX_REGISTERED_ID_BYTES, PlaybackMode};

        let schema = [definition(
            "reaction",
            SettingKind::Motion {
                default: String::new(),
            },
        )];
        // No catalog lookup: registered keys may become available again later.
        for id in [
            String::new(),
            "org.example.disabled/nod".into(),
            "x".repeat(MAX_REGISTERED_ID_BYTES),
            format!("{}x", "雪".repeat(MAX_REGISTERED_ID_BYTES / 3)),
        ] {
            let defaults = [definition(
                "reaction",
                SettingKind::Motion {
                    default: id.clone(),
                },
            )];
            validate_schema(&defaults).unwrap();
            let values = Settings::from([("reaction".into(), SettingValue::Text(id.clone()))]);
            assert_eq!(validate_values(&schema, &values).unwrap(), values);
            if !id.is_empty() {
                Action::MotionPlay {
                    motion_id: id,
                    mode: PlaybackMode::OneShot,
                }
                .validate()
                .unwrap();
            }
        }
        for id in [
            "x".repeat(MAX_REGISTERED_ID_BYTES + 1),
            format!("{}xx", "雪".repeat(MAX_REGISTERED_ID_BYTES / 3)),
            " ".into(),
            "a b".into(),
            " a".into(),
            "a ".into(),
            "a\u{a0}b".into(),
            "a\u{2003}b".into(),
            "a\u{2028}b".into(),
            "a\tb".into(),
            "a\nb".into(),
            "a\0b".into(),
            "a\u{7f}b".into(),
            "a\u{85}b".into(),
        ] {
            let defaults = [definition(
                "reaction",
                SettingKind::Motion {
                    default: id.clone(),
                },
            )];
            assert!(validate_schema(&defaults).is_err());
            let values = Settings::from([("reaction".into(), SettingValue::Text(id.clone()))]);
            assert!(validate_values(&schema, &values).is_err());
            assert!(
                Action::MotionPlay {
                    motion_id: id,
                    mode: PlaybackMode::OneShot
                }
                .validate()
                .is_err()
            );
            assert_eq!(
                migrate_values(&schema, &values).unwrap()["reaction"],
                SettingValue::Text(String::new())
            );
        }
        for value in [SettingValue::Bool(false), SettingValue::Integer(1)] {
            assert!(
                validate_values(&schema, &Settings::from([("reaction".into(), value)])).is_err()
            );
        }
    }

    #[test]
    fn motion_migration_keeps_unavailable_ids_and_explicit_none_defaults_invalid_values() {
        let schema = [definition(
            "reaction",
            SettingKind::Motion {
                default: "org.example.pack/nod".into(),
            },
        )];
        for id in ["org.example.disabled/gesture", ""] {
            let old = Settings::from([
                ("reaction".into(), SettingValue::Text(id.into())),
                ("removed".into(), SettingValue::Bool(true)),
            ]);
            let migrated = migrate_values(&schema, &old).unwrap();
            assert_eq!(
                migrated,
                Settings::from([("reaction".into(), SettingValue::Text(id.into()))])
            );
            assert_eq!(validate_values(&schema, &migrated).unwrap(), migrated);
        }
        let defaults = validate_values(&schema, &Settings::new()).unwrap();
        assert_eq!(migrate_values(&schema, &Settings::new()).unwrap(), defaults);
        for value in [
            SettingValue::Text("bad\u{2003}id".into()),
            SettingValue::Text("x".repeat(crate::extension_protocol::MAX_REGISTERED_ID_BYTES + 1)),
            SettingValue::Bool(true),
            SettingValue::Integer(2),
        ] {
            let old = Settings::from([("reaction".into(), value)]);
            assert_eq!(migrate_values(&schema, &old).unwrap(), defaults);
        }
    }

    #[test]
    fn expression_schema_is_strict_and_defaults_are_text_on_wire() {
        for default in ["", "Warm smile"] {
            let raw = json!({
                "id": "face", "label": "Expression", "type": "expression", "default": default,
            });
            let definition: SettingDefinition = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(
                definition.kind,
                SettingKind::Expression {
                    default: default.into()
                }
            );
            let serialized = serde_json::to_value(&definition).unwrap();
            assert_eq!(serialized["type"], "expression");
            assert_eq!(serialized["default"], default);
            assert_eq!(
                serde_json::from_value::<SettingDefinition>(serialized).unwrap(),
                definition
            );
            let defaults = validate_values(&[definition], &Settings::new()).unwrap();
            assert_eq!(defaults["face"], SettingValue::Text(default.into()));
            assert_eq!(
                serde_json::to_value(&defaults).unwrap(),
                json!({"face": default})
            );
            validate_wire_values(&defaults).unwrap();
            for field in ["unknown", "min", "max_length", "options"] {
                let mut invalid = raw.clone();
                invalid[field] = json!(null);
                assert!(serde_json::from_value::<SettingDefinition>(invalid).is_err());
            }
            for value in [json!(null), json!(false), json!(1), json!([])] {
                let mut invalid = raw.clone();
                invalid["default"] = value;
                assert!(serde_json::from_value::<SettingDefinition>(invalid).is_err());
            }
            let mut missing = raw;
            missing.as_object_mut().unwrap().remove("default");
            assert!(serde_json::from_value::<SettingDefinition>(missing).is_err());
        }
        assert!(serde_json::from_str::<SettingDefinition>(
            r#"{"id":"face","label":"Expression","type":"expression","default":"","default":"happy"}"#
        ).is_err());
    }

    #[test]
    fn expression_names_allow_spaces_and_none_with_bounded_nonreserved_defaults_and_values() {
        use crate::extension_protocol::{Action, MAX_ID_BYTES};

        let schema = [definition(
            "face",
            SettingKind::Expression {
                default: String::new(),
            },
        )];
        for name in [
            String::new(),
            "Unavailable custom smile".into(),
            " Warm smile ".into(),
            "Warm\u{2003}smile".into(),
            "x".repeat(MAX_ID_BYTES),
            format!("{}xx", "雪".repeat(MAX_ID_BYTES / 3)),
        ] {
            validate_schema(&[definition(
                "face",
                SettingKind::Expression {
                    default: name.clone(),
                },
            )])
            .unwrap();
            let values = Settings::from([("face".into(), SettingValue::Text(name.clone()))]);
            assert_eq!(validate_values(&schema, &values).unwrap(), values);
            if !name.is_empty() {
                Action::ExpressionApply {
                    name,
                    strength: 0.5,
                    duration_ms: 1000,
                }
                .validate()
                .unwrap();
            }
        }
        for name in [
            "x".repeat(MAX_ID_BYTES + 1),
            format!("{}xxx", "雪".repeat(MAX_ID_BYTES / 3)),
            " ".into(),
            "\u{a0}\u{2003}".into(),
            "warm\tsmile".into(),
            "warm\nsmile".into(),
            "warm\0smile".into(),
            "warm\u{7f}smile".into(),
            "warm\u{85}smile".into(),
            "BlinkLeft".into(),
            "LOOKUP".into(),
            "mouthSmile".into(),
            "aa".into(),
            "IH".into(),
            "ou".into(),
            "ee".into(),
            "oh".into(),
            "a".into(),
            "i".into(),
            "u".into(),
            "e".into(),
            "o".into(),
        ] {
            let invalid_schema = [definition(
                "face",
                SettingKind::Expression {
                    default: name.clone(),
                },
            )];
            assert!(validate_schema(&invalid_schema).is_err(), "{name:?}");
            assert!(migrate_values(&invalid_schema, &Settings::new()).is_err());
            let values = Settings::from([("face".into(), SettingValue::Text(name))]);
            assert!(validate_values(&schema, &values).is_err());
            assert_eq!(
                migrate_values(&schema, &values).unwrap()["face"],
                SettingValue::Text(String::new())
            );
        }
    }

    #[test]
    fn expression_migration_preserves_unavailable_names_and_none_defaults_reserved_or_mistyped() {
        let schema = [definition(
            "face",
            SettingKind::Expression {
                default: "Default smile".into(),
            },
        )];
        for name in ["Unavailable warm smile", ""] {
            let old = Settings::from([
                ("face".into(), SettingValue::Text(name.into())),
                ("removed".into(), SettingValue::Bool(true)),
            ]);
            let expected = Settings::from([("face".into(), SettingValue::Text(name.into()))]);
            assert_eq!(migrate_values(&schema, &old).unwrap(), expected);
            assert_eq!(validate_values(&schema, &expected).unwrap(), expected);
        }
        let defaults = validate_values(&schema, &Settings::new()).unwrap();
        assert_eq!(migrate_values(&schema, &Settings::new()).unwrap(), defaults);
        for value in [
            SettingValue::Text("blinkRight".into()),
            SettingValue::Text("\u{2003}".into()),
            SettingValue::Text("bad\nname".into()),
            SettingValue::Text("x".repeat(crate::extension_protocol::MAX_ID_BYTES + 1)),
            SettingValue::Bool(false),
            SettingValue::Integer(1),
        ] {
            let old = Settings::from([("face".into(), value)]);
            assert!(validate_values(&schema, &old).is_err());
            assert_eq!(migrate_values(&schema, &old).unwrap(), defaults);
        }
    }

    #[test]
    fn oversized_public_inputs_are_rejected_without_copying_into_diagnostics() {
        let values: Settings = [("x".repeat(1024 * 1024), SettingValue::Bool(false))].into();
        let error = validate_wire_values(&values).unwrap_err();
        assert!(error.len() < 256);
        let values: Settings =
            [("text".into(), SettingValue::Text("x".repeat(1024 * 1024)))].into();
        let error = validate_wire_values(&values).unwrap_err();
        assert!(error.len() < 256);
        let definitions = [definition(
            "text",
            SettingKind::Text {
                default: String::new(),
                max_length: MAX_STRING_BYTES,
            },
        )];
        // Migration checks compatibility before cloning an old value.
        assert_eq!(
            migrate_values(&definitions, &values).unwrap()["text"],
            SettingValue::Text(String::new())
        );
    }

    #[test]
    fn flattened_schema_is_strict_and_roundtrips_all_kinds() {
        for definition in schema() {
            let value = serde_json::to_value(&definition).unwrap();
            assert!(value.get("kind").is_none());
            assert!(value.get("type").is_some());
            assert_eq!(
                serde_json::from_value::<SettingDefinition>(value.clone()).unwrap(),
                definition
            );
            for field in ["unknown", "min", "max_length", "options"] {
                if value.get(field).is_some() {
                    continue;
                }
                let mut invalid = value.clone();
                invalid[field] = json!(null);
                assert!(
                    serde_json::from_value::<SettingDefinition>(invalid).is_err(),
                    "{field}"
                );
            }
        }
        let minimal: SettingDefinition = serde_json::from_value(json!({
            "id": "enabled", "label": "Enabled", "type": "toggle", "default": true,
        }))
        .unwrap();
        assert!(minimal.description.is_empty());
        for invalid in [
            r#"{"id":"a","id":"b","label":"A","type":"toggle","default":true}"#,
            r#"{"id":"a","label":"A","type":"toggle","default":true,"default":false}"#,
            r#"{"id":"a","label":"A","type":"toggle","type":"text","default":true}"#,
            r#"{"id":"a","label":"A","type":"secret","default":"key"}"#,
            r#"{"id":"a","label":"A","description":null,"type":"toggle","default":true}"#,
        ] {
            assert!(serde_json::from_str::<SettingDefinition>(invalid).is_err());
        }
        for invalid in [
            json!(1.5),
            json!(null),
            json!([]),
            json!({}),
            json!(u64::MAX),
        ] {
            assert!(serde_json::from_value::<SettingValue>(invalid).is_err());
        }
    }

    #[test]
    fn validation_fills_defaults_and_accepts_boundary_values_without_coercion() {
        let schema = schema();
        let defaults = validate_values(&schema, &Settings::new()).unwrap();
        assert_eq!(defaults["enabled"], SettingValue::Bool(true));
        assert_eq!(defaults["count"], SettingValue::Integer(3));
        let values: Settings = serde_json::from_value(json!({
            "enabled": false, "count": -2, "title": "雪雪ab", "mode": "normal",
        }))
        .unwrap();
        assert_eq!(validate_values(&schema, &values).unwrap(), values);
        for (id, value) in [
            ("missing", SettingValue::Bool(true)),
            ("enabled", SettingValue::Text("true".into())),
            ("count", SettingValue::Integer(11)),
            ("count", SettingValue::Bool(true)),
            ("title", SettingValue::Text("雪雪abc".into())),
            ("title", SettingValue::Text("new\nline".into())),
            ("mode", SettingValue::Text("missing".into())),
        ] {
            assert!(
                validate_values(&schema, &[(id.into(), value)].into()).is_err(),
                "{id}"
            );
        }
    }

    #[test]
    fn migration_keeps_compatible_values_defaults_invalid_and_drops_removed() {
        let old: Settings = serde_json::from_value(json!({
            "enabled": false, "count": 999, "title": "bad\n", "mode": "normal",
            "removed": "x".repeat(MAX_STRING_BYTES + 1),
        }))
        .unwrap();
        assert!(validate_wire_values(&old).is_err());
        let migrated = migrate_values(&schema(), &old).unwrap();
        assert_eq!(
            migrated,
            serde_json::from_value::<Settings>(json!({
                "enabled": false, "count": 3, "title": "hello", "mode": "normal",
            }))
            .unwrap()
        );
        assert_eq!(validate_values(&schema(), &migrated).unwrap(), migrated);
        assert!(migrate_values(&[], &old).unwrap().is_empty());
    }

    #[test]
    fn schema_checks_identity_labels_descriptions_ranges_and_defaults() {
        for id in [
            "",
            "UPPER",
            "-first",
            "a.b",
            "a/b",
            "snow雪",
            &"a".repeat(65),
        ] {
            assert!(
                validate_schema(&[definition(id, SettingKind::Toggle { default: true })]).is_err()
            );
        }
        let mut definitions = schema();
        definitions[1].id = definitions[0].id.clone();
        assert!(validate_schema(&definitions).is_err());
        assert!(validate_schema(&vec![schema()[0].clone(); MAX_SETTINGS + 1]).is_err());
        for kind in [
            SettingKind::Integer {
                default: 0,
                min: 2,
                max: 1,
            },
            SettingKind::Integer {
                default: 3,
                min: 0,
                max: 2,
            },
            SettingKind::Text {
                default: String::new(),
                max_length: 1025,
            },
            SettingKind::Text {
                default: "long".into(),
                max_length: 1,
            },
            SettingKind::Text {
                default: "\0".into(),
                max_length: 1,
            },
            SettingKind::Choice {
                default: "a".into(),
                options: vec![],
            },
            SettingKind::Choice {
                default: "a".into(),
                options: vec!["a".into(), "a".into()],
            },
            SettingKind::Choice {
                default: "a".into(),
                options: vec!["b".into()],
            },
            SettingKind::Choice {
                default: " ".into(),
                options: vec![" ".into()],
            },
            SettingKind::Choice {
                default: "a".into(),
                options: vec!["a".repeat(129)],
            },
            SettingKind::Choice {
                default: "a".into(),
                options: (0..33).map(|n| n.to_string()).collect(),
            },
        ] {
            assert!(validate_schema(&[definition("test", kind)]).is_err());
        }
        let mut definition = definition("valid", SettingKind::Toggle { default: true });
        definition.label = "l".repeat(MAX_LABEL_BYTES);
        definition.description = "d".repeat(MAX_DESCRIPTION_BYTES);
        validate_schema(&[definition.clone()]).unwrap();
        definition.label.push('x');
        assert!(validate_schema(&[definition.clone()]).is_err());
        definition.label = "Label".into();
        definition.description.push('x');
        assert!(validate_schema(&[definition]).is_err());
        validate_schema(&[self::definition(
            "empty",
            SettingKind::Text {
                default: String::new(),
                max_length: 0,
            },
        )])
        .unwrap();
    }

    #[test]
    fn wire_validation_bounds_counts_keys_controls_utf8_and_serialized_expansion() {
        let full: Settings = (0..MAX_SETTINGS)
            .map(|n| (format!("s{n}"), SettingValue::Integer(i64::MIN)))
            .collect();
        validate_wire_values(&full).unwrap();
        let mut too_many = full;
        too_many.insert("extra".into(), SettingValue::Bool(true));
        assert!(validate_wire_values(&too_many).is_err());
        for (id, value) in [
            ("bad.id", SettingValue::Bool(false)),
            ("good", SettingValue::Text("x".repeat(MAX_STRING_BYTES + 1))),
            ("good", SettingValue::Text("a\tb".into())),
            ("good", SettingValue::Text("a\u{7f}b".into())),
            ("good", SettingValue::Text("a\u{85}b".into())),
        ] {
            assert!(validate_wire_values(&[(id.into(), value)].into()).is_err());
        }
        let utf8: Settings = [("title".into(), SettingValue::Text("雪".repeat(341)))].into();
        validate_wire_values(&utf8).unwrap();
        // All individual strings fit, but escaping doubles quote/backslash bytes.
        let escaped: Settings = (0..9)
            .map(|n| (format!("s{n}"), SettingValue::Text("\"".repeat(1024))))
            .collect();
        assert!(
            validate_wire_values(&escaped)
                .unwrap_err()
                .contains("serialized")
        );
        let huge_defaults: Vec<_> = (0..17)
            .map(|n| {
                definition(
                    &format!("s{n}"),
                    SettingKind::Text {
                        default: "x".repeat(1024),
                        max_length: 1024,
                    },
                )
            })
            .collect();
        assert!(validate_schema(&huge_defaults).is_err());
    }

    #[test]
    fn complete_values_and_migration_enforce_total_budget_after_filling_defaults() {
        let definitions: Vec<_> = (0..17)
            .map(|n| {
                definition(
                    &format!("s{n}"),
                    SettingKind::Text {
                        default: String::new(),
                        max_length: 1024,
                    },
                )
            })
            .collect();
        validate_schema(&definitions).unwrap();
        let old: Settings = (0..17)
            .map(|n| (format!("s{n}"), SettingValue::Text("x".repeat(1024))))
            .collect();
        assert!(validate_values(&definitions, &old).is_err());
        assert!(migrate_values(&definitions, &old).is_err());
        // Each partial input fits but filling missing nonempty defaults can fail.
        let definitions: Vec<_> = (0..16)
            .map(|n| {
                definition(
                    &format!("s{n}"),
                    SettingKind::Text {
                        default: "x".repeat(500),
                        max_length: 1024,
                    },
                )
            })
            .collect();
        let input: Settings = (0..15)
            .map(|n| (format!("s{n}"), SettingValue::Text("x".repeat(1024))))
            .collect();
        validate_wire_values(&input).unwrap();
        // Escaped content reaches the bound with fewer populated keys.
        let input: Settings = (0..8)
            .map(|n| (format!("s{n}"), SettingValue::Text("\"".repeat(1000))))
            .collect();
        validate_wire_values(&input).unwrap();
        assert!(validate_values(&definitions, &input).is_err());
    }
}
