//! Lightweight, renderer-free preflight of binary VRM files.
use serde_json::Value;
use std::{collections::BTreeMap, fmt, path::Path};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VrmVersion {
    V0,
    V1,
}

impl fmt::Display for VrmVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::V0 => "0.0",
            Self::V1 => "1.0",
        })
    }
}

#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub version: VrmVersion,
    pub name: String,
    pub authors: Vec<String>,
    pub nodes: usize,
    pub meshes: usize,
    pub materials: usize,
    pub humanoid_bones: BTreeMap<String, usize>,
    /// Original metadata, including the model's usage and license conditions.
    pub meta: Value,
}

#[derive(Debug, Error)]
pub enum InspectError {
    #[error("Cannot read model: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid VRM: {0}")]
    Invalid(String),
    #[error("Invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn inspect_file(path: impl AsRef<Path>) -> Result<ModelInfo, InspectError> {
    inspect_bytes(&std::fs::read(path)?)
}

pub fn inspect_bytes(bytes: &[u8]) -> Result<ModelInfo, InspectError> {
    let invalid = |reason: &str| InspectError::Invalid(reason.to_owned());
    if bytes.len() < 20 || &bytes[..4] != b"glTF" {
        return Err(invalid("expected a binary glTF 2.0 container (.vrm)"));
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if version != 2 || length != bytes.len() {
        return Err(invalid("incorrect GLB version or declared length"));
    }
    let glb =
        gltf::binary::Glb::from_slice(bytes).map_err(|e| InspectError::Invalid(e.to_string()))?;
    let doc: Value = serde_json::from_slice(&glb.json)?;
    if doc.pointer("/asset/version").and_then(Value::as_str) != Some("2.0") {
        return Err(invalid("asset.version must be 2.0"));
    }
    let v0 = doc.pointer("/extensions/VRM");
    let v1 = doc.pointer("/extensions/VRMC_vrm");
    let (version, ext) = match (v0, v1) {
        (Some(ext), None) if ext.is_object() => (VrmVersion::V0, ext),
        (None, Some(ext)) if ext.is_object() => (VrmVersion::V1, ext),
        (Some(_), Some(_)) => return Err(invalid("ambiguous VRM 0.0 and 1.0 extensions")),
        _ => return Err(invalid("missing VRM or VRMC_vrm extension")),
    };
    let expected = version.to_string();
    if ext.get("specVersion").and_then(Value::as_str) != Some(expected.as_str()) {
        return Err(invalid("unsupported or missing VRM specVersion"));
    }
    let count = |key: &str| doc.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    let nodes = count("nodes");
    if count("scenes") == 0 || count("meshes") == 0 {
        return Err(invalid("model has no scene or meshes"));
    }
    let mut humanoid_bones = BTreeMap::new();
    let mut insert = |name: &str, node: &Value| -> Result<(), InspectError> {
        let index = node
            .as_u64()
            .ok_or_else(|| invalid("invalid humanoid node index"))?;
        if index >= nodes as u64 {
            return Err(invalid("humanoid bone references a missing node"));
        }
        if humanoid_bones
            .insert(name.to_owned(), index as usize)
            .is_some()
        {
            return Err(invalid("duplicate humanoid bone name"));
        }
        Ok(())
    };
    let bones = ext.pointer("/humanoid/humanBones");
    match version {
        VrmVersion::V0 => {
            for bone in bones
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("missing humanoid bones"))?
            {
                let name = bone
                    .get("bone")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("missing bone name"))?;
                insert(name, &bone["node"])?;
            }
        }
        VrmVersion::V1 => {
            for (name, bone) in bones
                .and_then(Value::as_object)
                .ok_or_else(|| invalid("missing humanoid bones"))?
            {
                insert(name, &bone["node"])?;
            }
        }
    }
    if !humanoid_bones.contains_key("hips") {
        return Err(invalid("humanoid has no hips bone"));
    }
    let meta = ext.get("meta").cloned().unwrap_or(Value::Null);
    let name_key = if version == VrmVersion::V0 {
        "title"
    } else {
        "name"
    };
    let name = meta
        .get(name_key)
        .and_then(Value::as_str)
        .unwrap_or("Untitled avatar")
        .to_owned();
    let authors = if version == VrmVersion::V0 {
        meta.get("author")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .into_iter()
            .collect()
    } else {
        meta.get("authors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    Ok(ModelInfo {
        version,
        name,
        authors,
        nodes,
        meshes: count("meshes"),
        materials: count("materials"),
        humanoid_bones,
        meta,
    })
}
