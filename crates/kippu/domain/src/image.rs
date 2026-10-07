//! Event images. The bytes live in object storage; the database keeps what is known about them.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{EventId, ImageId, Timestamp, ValidationError};

/// The image formats accepted, recognized by their bytes rather than by what the client claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum ImageFormat {
    /// `image/png`
    #[serde(rename = "image/png")]
    Png,
    /// `image/jpeg`
    #[serde(rename = "image/jpeg")]
    Jpeg,
    /// `image/webp`
    #[serde(rename = "image/webp")]
    Webp,
    /// `image/avif`
    #[serde(rename = "image/avif")]
    Avif,
}

impl ImageFormat {
    /// Every accepted format.
    pub const ALL: [Self; 4] = [Self::Png, Self::Jpeg, Self::Webp, Self::Avif];

    /// The media type, e.g. `image/png`.
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
        }
    }

    /// The usual file extension.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Avif => "avif",
        }
    }

    /// The format the bytes are in, judged by their signature, if it is an accepted one.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(Self::Jpeg)
        } else if bytes.len() >= 12
            && bytes.starts_with(b"RIFF")
            && bytes.get(8..12) == Some(b"WEBP")
        {
            Some(Self::Webp)
        } else if matches!(bytes.get(4..12), Some(b"ftypavif" | b"ftypavis")) {
            Some(Self::Avif)
        } else {
            None
        }
    }
}

impl fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.media_type())
    }
}

impl FromStr for ImageFormat {
    type Err = ValidationError;

    /// Parses a media type; parameters such as `; charset=…` are ignored.
    fn from_str(media_type: &str) -> Result<Self, Self::Err> {
        let essence = media_type.split(';').next().unwrap_or_default().trim();
        Self::ALL
            .into_iter()
            .find(|format| format.media_type().eq_ignore_ascii_case(essence))
            .ok_or_else(|| {
                ValidationError::new("Content-Type", "must be image/png, jpeg, webp or avif")
            })
    }
}

/// An image of an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EventImage {
    /// Identity of the image.
    pub id: ImageId,
    /// The event it shows.
    pub event_id: EventId,
    /// Its format.
    pub format: ImageFormat,
    /// Its size in bytes.
    pub size_bytes: u64,
    /// Where it comes in the event's gallery; lower first.
    pub position: u32,
    /// When it was uploaded.
    pub created_at: Timestamp,
}

impl EventImage {
    /// Where the bytes are kept in object storage, relative to the configured prefix.
    pub fn object_key(&self) -> String {
        format!(
            "events/{}/images/{}.{}",
            self.event_id,
            self.id,
            self.format.extension()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_recognized_by_their_bytes() {
        assert_eq!(
            ImageFormat::sniff(b"\x89PNG\r\n\x1a\nrest"),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            ImageFormat::sniff(&[0xff, 0xd8, 0xff, 0xe0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            ImageFormat::sniff(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageFormat::Webp)
        );
        assert_eq!(
            ImageFormat::sniff(b"\0\0\0\x1cftypavif"),
            Some(ImageFormat::Avif)
        );
        assert_eq!(ImageFormat::sniff(b"<svg xmlns="), None);
        assert_eq!(ImageFormat::sniff(b""), None);
    }

    #[test]
    fn media_types_parse_without_parameters() {
        assert_eq!(
            "image/PNG".parse::<ImageFormat>().unwrap(),
            ImageFormat::Png
        );
        assert_eq!(
            "image/jpeg; q=1".parse::<ImageFormat>().unwrap(),
            ImageFormat::Jpeg
        );
        assert!("image/svg+xml".parse::<ImageFormat>().is_err());
    }
}
