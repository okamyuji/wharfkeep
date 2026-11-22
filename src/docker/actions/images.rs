//! イメージの取得と整理。

use super::{filter, reclaimed, to_json};
use crate::docker::{DockerError, Engine, check_image_ref, size_to_option};
use bollard::models::CreateImageInfo;
use bollard::query_parameters::{
    CreateImageOptionsBuilder, PruneImagesOptionsBuilder, RemoveImageOptionsBuilder,
    TagImageOptionsBuilder,
};
use futures_util::{Stream, StreamExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullProgress {
    pub id: Option<String>,
    pub status: String,
    pub current: Option<u64>,
    pub total: Option<u64>,
}

fn pull_progress(info: CreateImageInfo) -> Result<PullProgress, DockerError> {
    if let Some(e) = info.error_detail {
        return Err(DockerError::Api(e.message.unwrap_or_default()));
    }
    let detail = info.progress_detail.unwrap_or_default();
    Ok(PullProgress {
        id: info.id,
        status: info.status.unwrap_or_default(),
        current: detail.current.and_then(size_to_option),
        total: detail.total.and_then(size_to_option),
    })
}

impl Engine {
    pub fn pull(
        &self,
        reference: &str,
    ) -> impl Stream<Item = Result<PullProgress, DockerError>> + '_ {
        let opts = CreateImageOptionsBuilder::new()
            .from_image(reference)
            .build();
        self.docker
            .create_image(Some(opts), None, None)
            .map(|r| pull_progress(r?))
    }

    pub async fn remove_image(&self, id: &str, force: bool) -> Result<(), DockerError> {
        check_image_ref(id)?;
        let opts = RemoveImageOptionsBuilder::new().force(force).build();
        self.docker.remove_image(id, Some(opts), None).await?;
        Ok(())
    }

    pub async fn tag(&self, id: &str, repo: &str, tag: &str) -> Result<(), DockerError> {
        check_image_ref(id)?;
        check_image_ref(repo)?;
        let opts = TagImageOptionsBuilder::new().repo(repo).tag(tag).build();
        Ok(self.docker.tag_image(id, Some(opts)).await?)
    }

    /// `dangling_only` が偽のときは、使われていないイメージをすべて消す。
    pub async fn prune_images(&self, dangling_only: bool) -> Result<u64, DockerError> {
        let filters = filter("dangling", if dangling_only { "true" } else { "false" });
        let opts = PruneImagesOptionsBuilder::new().filters(&filters).build();
        let r = self.docker.prune_images(Some(opts)).await?;
        Ok(reclaimed(r.space_reclaimed))
    }

    pub async fn inspect_image(&self, id: &str) -> Result<serde_json::Value, DockerError> {
        check_image_ref(id)?;
        to_json(&self.docker.inspect_image(id).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_progress_maps_status_and_error() {
        let ok = pull_progress(CreateImageInfo {
            id: Some("abc".into()),
            status: Some("Downloading".into()),
            progress_detail: Some(bollard::models::ProgressDetail {
                current: Some(5),
                total: Some(10),
            }),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            ok,
            PullProgress {
                id: Some("abc".into()),
                status: "Downloading".into(),
                current: Some(5),
                total: Some(10)
            }
        );
        let bare = pull_progress(CreateImageInfo::default()).unwrap();
        assert_eq!(bare.status, "");
        assert_eq!(bare.current, None);
        let err = pull_progress(CreateImageInfo {
            error_detail: Some(bollard::models::ErrorDetail {
                code: None,
                message: Some("denied".into()),
            }),
            ..Default::default()
        });
        assert_eq!(err, Err(DockerError::Api("denied".into())));
    }
}
