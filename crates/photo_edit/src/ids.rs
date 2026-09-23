use uuid::Uuid;

pub fn new_id() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"];

pub fn is_image_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            IMAGE_EXTS.iter().any(|x| e == *x)
        })
        .unwrap_or(false)
}
