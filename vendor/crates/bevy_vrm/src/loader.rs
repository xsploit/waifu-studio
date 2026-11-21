//! Asset loader reading `.vrm` files as glTF.

use std::{borrow::Cow, io::Cursor};

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    gltf::{Gltf, GltfError, GltfLoader, GltfLoaderSettings},
    prelude::*,
};
use gltf::binary::Glb;
use image::{ImageFormat, ImageReader, Limits};
use serde_json::Value;
use thiserror::Error;

#[derive(Asset, TypePath, Debug)]
pub struct Vrm {
    pub gltf: Gltf,
}

#[derive(TypePath)]
pub struct VrmLoader {
    pub gltf_loader: GltfLoader,
}

#[derive(Debug, Error)]
pub enum VrmError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Gltf(#[from] GltfError),
}

impl AssetLoader for VrmLoader {
    type Asset = Vrm;
    type Settings = GltfLoaderSettings;
    type Error = VrmError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Vrm, VrmError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let repaired = convert_embedded_gifs(&bytes);
        let bytes = if let Some((repaired, count)) = repaired {
            warn!(
                "Converted {count} embedded VRM GIF texture(s) to static PNG in {}",
                load_context.path()
            );
            repaired
        } else {
            bytes
        };
        let gltf = GltfLoader::load_gltf(&self.gltf_loader, &bytes, load_context, settings).await?;

        Ok(Vrm { gltf })
    }

    fn extensions(&self) -> &[&str] {
        &["vrm"]
    }
}

fn convert_embedded_gifs(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    // Some VRM exporters write GIF payloads where glTF expects static textures.
    if u32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?) < 12 {
        return None;
    }
    let mut glb = Glb::from_slice(bytes).ok()?;
    let bin = glb.bin.as_deref()?;
    let mut document: Value = serde_json::from_slice(&glb.json).ok()?;
    let views = document.get("bufferViews")?.as_array()?;
    let images = document.get("images")?.as_array()?;
    let replacements: Vec<_> = images
        .iter()
        .enumerate()
        .filter_map(|(index, image)| {
            let view_id = usize::try_from(image.get("bufferView")?.as_u64()?).ok()?;
            let view = views.get(view_id)?;
            if view.get("buffer")?.as_u64()? != 0 {
                return None;
            }
            let offset =
                usize::try_from(view.get("byteOffset").map_or(Some(0), Value::as_u64)?).ok()?;
            let length = usize::try_from(view.get("byteLength")?.as_u64()?).ok()?;
            let png = gif_first_frame_png(bin.get(offset..offset.checked_add(length)?)?)?;
            Some((index, png))
        })
        .collect();
    if replacements.is_empty() || document.get("buffers")?.get(0)?.get("uri").is_some() {
        return None;
    }
    let count = replacements.len();
    let bin = glb.bin.as_mut()?.to_mut();
    // Append fresh views so every original buffer offset and mesh stays intact.
    for (image, png) in replacements {
        let offset = bin.len().next_multiple_of(4);
        let new_len = offset.checked_add(png.len())?;
        u32::try_from(new_len).ok()?;
        let views = document.get_mut("bufferViews")?.as_array_mut()?;
        let view = views.len();
        views.push(serde_json::json!({"buffer": 0, "byteOffset": offset, "byteLength": png.len()}));
        let image = document
            .get_mut("images")?
            .get_mut(image)?
            .as_object_mut()?;
        image.insert("bufferView".into(), view.into());
        image.insert("mimeType".into(), "image/png".into());
        bin.resize(offset, 0);
        bin.extend_from_slice(&png);
    }
    document
        .get_mut("buffers")?
        .get_mut(0)?
        .as_object_mut()?
        .insert("byteLength".into(), bin.len().into());
    let json = serde_json::to_vec(&document).ok()?;
    u32::try_from(
        28usize
            .checked_add(bin.len().next_multiple_of(4))?
            .checked_add(json.len().next_multiple_of(4))?,
    )
    .ok()?;
    glb.json = Cow::Owned(json);
    Some((glb.to_vec().ok()?, count))
}

fn gif_first_frame_png(bytes: &[u8]) -> Option<Vec<u8>> {
    if !matches!(bytes, [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..]) {
        return None;
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Gif);
    let mut limits = Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut png = Cursor::new(Vec::new());
    reader
        .decode()
        .ok()?
        .write_to(&mut png, ImageFormat::Png)
        .ok()?;
    Some(png.into_inner())
}
