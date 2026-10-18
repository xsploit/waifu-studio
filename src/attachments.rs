//! Explicitly selected local images only. No screen capture or persistent image history.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{ImageFormat, ImageReader};
use std::{
    io::{Cursor, Read},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};

pub const MAX_ATTACHMENTS: usize = 2;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_DATA_URL_BYTES: usize = 9 * 1024 * 1024;
#[derive(Clone)]
pub struct ImageAttachment {
    pub name: String,
    pub data_url: String,
}
impl std::fmt::Debug for ImageAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageAttachment")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}
static PREPARING: AtomicBool = AtomicBool::new(false);
struct PreparationPermit;
impl Drop for PreparationPermit {
    fn drop(&mut self) {
        PREPARING.store(false, Ordering::Release);
    }
}
#[derive(Default)]
pub struct AttachmentWorker {
    receiver: Option<Receiver<Result<ImageAttachment, String>>>,
}
impl AttachmentWorker {
    pub fn is_busy(&self) -> bool {
        self.receiver.is_some()
    }
    pub fn start(&mut self, path: PathBuf) -> Result<(), String> {
        if self.is_busy() {
            return Err("An image is already being prepared".into());
        }
        if PREPARING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("An image is already being prepared".into());
        }
        let permit = PreparationPermit;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("studio-image".into())
            .spawn(move || {
                let _permit = permit;
                let _ = tx.send(prepare_image(path));
            })
            .map_err(|_| "Cannot start image preparation worker")?;
        self.receiver = Some(rx);
        Ok(())
    }
    pub fn poll(&mut self) -> Option<Result<ImageAttachment, String>> {
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(result) => {
                self.receiver = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                Some(Err("Image preparation stopped unexpectedly".into()))
            }
        }
    }
}
fn prepare_image(path: PathBuf) -> Result<ImageAttachment, String> {
    // Inspect before opening: opening a FIFO can otherwise block forever and
    // hold the single preparation permit. Follow symlinks to check the target.
    let before = std::fs::metadata(&path).map_err(|_| "Cannot inspect selected image")?;
    if !before.is_file() || before.len() > MAX_FILE_BYTES {
        return Err("Choose an image file no larger than 8 MiB".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A path replaced with a FIFO between metadata and open must not block.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .map_err(|_| "Cannot open selected image")?;
    let metadata = file
        .metadata()
        .map_err(|_| "Cannot inspect selected image")?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err("Choose an image file no larger than 8 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read selected image")?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Image exceeds 8 MiB".into());
    }
    let format = image::guess_format(&bytes).map_err(|_| "Choose a PNG, JPEG, or WebP image")?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err("Choose a PNG, JPEG, or WebP image".into());
    }
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| "Image could not be decoded within the size limits")?;
    if decoded.width() == 0 || decoded.height() == 0 {
        return Err("Image is empty".into());
    }
    let resized = decoded.thumbnail(1280, 1280).to_rgb8();
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 85)
        .encode_image(&resized)
        .map_err(|_| "Cannot encode selected image")?;
    let data_url = format!("data:image/jpeg;base64,{}", STANDARD.encode(encoded));
    if data_url.len() > MAX_DATA_URL_BYTES {
        return Err("Prepared image is too large".into());
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect();
    Ok(ImageAttachment { name, data_url })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_preparation_resizes_and_discards_original_metadata() {
        let path = std::env::temp_dir().join(format!("studio-image-{}.png", uuid::Uuid::new_v4()));
        image::RgbImage::new(2000, 100).save(&path).unwrap();
        let attachment = prepare_image(path.clone()).unwrap();
        std::fs::remove_file(path).unwrap();
        let bytes = STANDARD
            .decode(attachment.data_url.split_once(',').unwrap().1)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!(image.width(), 1280);
        assert!(image.height() <= 1280);
        assert!(!format!("{attachment:?}").contains("base64"));
    }
    #[cfg(unix)]
    #[test]
    fn rejects_special_files_before_opening() {
        let path =
            std::env::temp_dir().join(format!("studio-special-{}.png", uuid::Uuid::new_v4()));
        let socket = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert!(prepare_image(path.clone()).unwrap_err().contains("8 MiB"));
        drop(socket);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn rejects_oversized_and_non_image_files() {
        let path = std::env::temp_dir().join(format!("studio-image-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"not an image").unwrap();
        assert!(prepare_image(path.clone()).is_err());
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(prepare_image(path.clone()).unwrap_err().contains("8 MiB"));
        std::fs::remove_file(path).unwrap();
    }
}
