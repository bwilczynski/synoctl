//! `docker images`, from homelab-api/internal/docker/images_service.go.

use jiff::Timestamp;

use crate::dsm::types::Image;
use crate::error::{Error, Result};
use crate::model::docker::{DockerImage, DockerImageDetail, DockerImageList};
use crate::service::Service;

impl Service {
    pub fn list_images(&self) -> Result<DockerImageList> {
        let items = self.api.images()?.images.iter().map(image).collect();
        Ok(DockerImageList { items })
    }

    /// By short ID: the image whose digest starts with it.
    pub fn get_image(&self, id: &str) -> Result<DockerImageDetail> {
        let not_found = || Error::NotFound(format!("Image not found: {id}"));
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(not_found());
        }
        let images = self.api.images()?.images;
        let img = images
            .iter()
            .find(|i| i.id.starts_with(&format!("sha256:{id}")))
            .ok_or_else(not_found)?;
        Ok(DockerImageDetail {
            image: image(img),
            virtual_size: img.virtual_size,
            created: Timestamp::from_second(img.created).unwrap_or(Timestamp::UNIX_EPOCH),
            description: (!img.description.is_empty()).then(|| img.description.clone()),
        })
    }
}

fn image(i: &Image) -> DockerImage {
    DockerImage {
        id: short_id(&i.id),
        repository: i.repository.clone(),
        tags: i.tags.clone(),
        size: i.size,
    }
}

/// The digest without `sha256:`, cut to 12 characters.
pub(crate) fn short_id(full: &str) -> String {
    let s = full.strip_prefix("sha256:").unwrap_or(full);
    s.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::model::Ids;
    use crate::model::docker::DockerImage;
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn list_images() {
        let l = svc(fixtures()).list_images().unwrap();
        assert_eq!(
            l.ids(),
            [
                "aabbccddeeff",
                "bbccddeeff00",
                "ccddeeff0011",
                "ddeeff001122"
            ]
        );
        assert_eq!(
            l.items[1],
            DockerImage {
                id: "bbccddeeff00".into(),
                repository: "gcr.io/cadvisor/cadvisor".into(),
                tags: vec!["v0.52.1".into()],
                size: 80729315
            }
        );
    }

    #[test]
    fn get_image_detail() {
        let d = svc(fixtures()).get_image("bbccddeeff00").unwrap();
        assert_eq!(d.created, Timestamp::from_second(1742442027).unwrap());
        assert_eq!(d.virtual_size, 80729315);
        assert_eq!(d.description, None);
        let v = serde_json::to_value(&d).unwrap();
        assert!(v.get("description").is_none());
        assert_eq!(v["virtualSize"], 80729315);
    }

    #[test]
    fn description_kept_when_present() {
        let mut api = fixtures();
        api.images.images[0].description = "busybox image".into();
        let d = svc(api).get_image("aabbccddeeff").unwrap();
        assert_eq!(d.description.as_deref(), Some("busybox image"));
    }

    #[test]
    fn image_not_found_and_empty_id() {
        for id in ["000000000000", "", "nas-01.aabbccddeeff"] {
            assert_eq!(
                svc(fixtures()).get_image(id).unwrap_err().to_string(),
                format!("Image not found: {id}")
            );
        }
    }

    #[test]
    fn short_id_strips_prefix() {
        assert_eq!(short_id("sha256:0123456789abcdef"), "0123456789ab");
        assert_eq!(short_id("abc"), "abc");
    }
}
