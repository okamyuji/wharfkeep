//! イメージの取得と整理。

use super::{filter, reclaimed, to_json};
use crate::docker::{DockerError, Engine, check_image_ref, check_repo, check_tag, size_to_option};
use bollard::models::CreateImageInfo;
use bollard::query_parameters::{
    CreateImageOptionsBuilder, PruneBuildOptionsBuilder, PruneImagesOptionsBuilder,
    RemoveImageOptionsBuilder, TagImageOptionsBuilder,
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

    /// イメージ本体まで消えたら`true`。ほかのタグが残ってタグだけ外れたら`false`。
    pub async fn remove_image(&self, id: &str, force: bool) -> Result<bool, DockerError> {
        check_image_ref(id)?;
        let opts = RemoveImageOptionsBuilder::new().force(force).build();
        let items = self.docker.remove_image(id, Some(opts), None).await?;
        Ok(items.iter().any(|i| i.deleted.is_some()))
    }

    pub async fn tag(&self, id: &str, repo: &str, tag: &str) -> Result<(), DockerError> {
        check_image_ref(id)?;
        check_repo(repo)?;
        check_tag(tag)?;
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

    /// 使われていないビルドキャッシュをすべて消す。`all` を付けないと、ディスク使用量に数えるキャッシュの一部が残る。
    pub async fn prune_build_cache(&self) -> Result<u64, DockerError> {
        let opts = PruneBuildOptionsBuilder::new().all(true).build();
        let r = self.docker.prune_build(Some(opts)).await?;
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

    use crate::docker::actions::fake::{fake_docker, route};

    #[tokio::test]
    async fn prune_build_cache_posts_build_prune() {
        let f = fake_docker(vec![route(
            "/build/prune",
            r#"{"CachesDeleted":["a"],"SpaceReclaimed":1234}"#,
        )]);
        assert_eq!(f.engine.prune_build_cache().await, Ok(1234));
        let seen = f.seen();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].starts_with("POST /"), "{}", seen[0]);
        assert!(seen[0].contains("/build/prune?all=true"), "{}", seen[0]);
    }

    #[tokio::test]
    async fn prune_build_cache_without_a_size_is_zero_and_errors_pass_through() {
        let f = fake_docker(vec![route("/build/prune", "{}")]);
        assert_eq!(f.engine.prune_build_cache().await, Ok(0));
        let f = fake_docker(vec![]);
        assert!(matches!(
            f.engine.prune_build_cache().await,
            Err(DockerError::Api(_))
        ));
    }

    #[tokio::test]
    async fn prune_images_sends_the_dangling_filter_and_returns_the_size() {
        for (dangling, want) in [(true, "%22true%22"), (false, "%22false%22")] {
            let f = fake_docker(vec![route(
                "/images/prune",
                r#"{"ImagesDeleted":[],"SpaceReclaimed":42}"#,
            )]);
            assert_eq!(f.engine.prune_images(dangling).await, Ok(42));
            let seen = f.seen();
            assert!(seen[0].starts_with("POST /"), "{}", seen[0]);
            assert!(
                seen[0].contains("%22dangling%22%3A%5B") && seen[0].contains(want),
                "{}",
                seen[0]
            );
        }
        let f = fake_docker(vec![route("/images/prune", "{}")]);
        assert_eq!(f.engine.prune_images(true).await, Ok(0));
        let f = fake_docker(vec![]);
        assert!(f.engine.prune_images(true).await.is_err());
    }

    #[tokio::test]
    async fn remove_image_says_whether_the_image_itself_was_deleted() {
        let f = fake_docker(vec![route(
            "/images/nginx:alpine",
            r#"[{"Untagged":"nginx:alpine"},{"Deleted":"sha256:aa"}]"#,
        )]);
        assert_eq!(f.engine.remove_image("nginx:alpine", false).await, Ok(true));
        let f = fake_docker(vec![route(
            "/images/nginx:alpine",
            r#"[{"Untagged":"nginx:alpine"}]"#,
        )]);
        assert_eq!(
            f.engine.remove_image("nginx:alpine", false).await,
            Ok(false)
        );
    }

    #[tokio::test]
    async fn remove_image_sends_delete_with_the_force_flag() {
        for (force, want) in [(true, "force=true"), (false, "force=false")] {
            let f = fake_docker(vec![route("/images/nginx:alpine", "[]")]);
            assert_eq!(
                f.engine.remove_image("nginx:alpine", force).await,
                Ok(false)
            );
            let seen = f.seen();
            assert!(seen[0].starts_with("DELETE /"), "{}", seen[0]);
            assert!(seen[0].contains(want), "{}", seen[0]);
        }
    }
}
