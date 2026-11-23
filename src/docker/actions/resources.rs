//! ボリュームとネットワークの操作。

use super::{reclaimed, to_json};
use crate::docker::actions::check_volume_name;
use crate::docker::{DockerError, Engine, check_name};
use bollard::models::{NetworkCreateRequest, VolumeCreateRequest};
use bollard::query_parameters::{
    InspectNetworkOptions, PruneNetworksOptions, PruneVolumesOptions, RemoveVolumeOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetDriver {
    Bridge,
    Macvlan,
    Ipvlan,
}

impl NetDriver {
    pub const ALL: [NetDriver; 3] = [NetDriver::Bridge, NetDriver::Macvlan, NetDriver::Ipvlan];

    pub fn as_str(self) -> &'static str {
        match self {
            NetDriver::Bridge => "bridge",
            NetDriver::Macvlan => "macvlan",
            NetDriver::Ipvlan => "ipvlan",
        }
    }
}

impl Engine {
    pub async fn create_volume(&self, name: &str) -> Result<(), DockerError> {
        check_volume_name(name)?;
        let req = VolumeCreateRequest {
            name: Some(name.to_string()),
            ..Default::default()
        };
        self.docker.create_volume(req).await?;
        Ok(())
    }

    pub async fn remove_volume(&self, name: &str) -> Result<(), DockerError> {
        check_name(name)?;
        Ok(self
            .docker
            .remove_volume(name, None::<RemoveVolumeOptions>)
            .await?)
    }

    /// 使われていない無名ボリュームを消す。名前付きは消さない（`docker volume prune` と同じ）。
    pub async fn prune_volumes(&self) -> Result<u64, DockerError> {
        let r = self
            .docker
            .prune_volumes(None::<PruneVolumesOptions>)
            .await?;
        Ok(reclaimed(r.space_reclaimed))
    }

    pub async fn inspect_volume(&self, name: &str) -> Result<serde_json::Value, DockerError> {
        check_name(name)?;
        to_json(&self.docker.inspect_volume(name).await?)
    }

    pub async fn create_network(&self, name: &str, driver: NetDriver) -> Result<(), DockerError> {
        let req = NetworkCreateRequest {
            name: name.to_string(),
            driver: Some(driver.as_str().to_string()),
            ..Default::default()
        };
        self.docker.create_network(req).await?;
        Ok(())
    }

    pub async fn remove_network(&self, id: &str) -> Result<(), DockerError> {
        check_name(id)?;
        Ok(self.docker.remove_network(id).await?)
    }

    /// 消したネットワークの数を返す。
    pub async fn prune_networks(&self) -> Result<usize, DockerError> {
        let r = self
            .docker
            .prune_networks(None::<PruneNetworksOptions>)
            .await?;
        Ok(r.networks_deleted.map_or(0, |n| n.len()))
    }

    pub async fn inspect_network(&self, id: &str) -> Result<serde_json::Value, DockerError> {
        check_name(id)?;
        let r = self
            .docker
            .inspect_network(id, None::<InspectNetworkOptions>)
            .await?;
        to_json(&r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_driver_names() {
        assert_eq!(NetDriver::Bridge.as_str(), "bridge");
        assert_eq!(NetDriver::Macvlan.as_str(), "macvlan");
        assert_eq!(NetDriver::Ipvlan.as_str(), "ipvlan");
    }

    use crate::docker::actions::fake::{fake_docker, route};

    fn body_of(request: &str) -> serde_json::Value {
        serde_json::from_str(request.split_once('\n').unwrap().1).unwrap()
    }

    #[tokio::test]
    async fn create_volume_posts_the_name() {
        let f = fake_docker(vec![route(
            "/volumes/create",
            r#"{"Name":"data-1","Driver":"local","Mountpoint":"/m","Labels":{},"Options":{},"Scope":"local"}"#,
        )]);
        assert_eq!(f.engine.create_volume("data-1").await, Ok(()));
        let seen = f.seen();
        assert!(seen[0].starts_with("POST /"), "{}", seen[0]);
        assert_eq!(body_of(&seen[0])["Name"], "data-1");
        let f = fake_docker(vec![]);
        assert!(f.engine.create_volume("data-1").await.is_err());
    }

    #[tokio::test]
    async fn create_volume_refuses_a_name_over_255_characters_before_calling_docker() {
        let f = fake_docker(vec![]);
        assert_eq!(
            f.engine.create_volume(&"v".repeat(256)).await,
            Err(DockerError::VolumeNameTooLong)
        );
        assert_eq!(
            f.engine.create_volume("a/b").await,
            Err(DockerError::InvalidId("a/b".into()))
        );
        assert!(f.seen().is_empty());
    }

    #[tokio::test]
    async fn create_network_posts_the_name_with_spaces_and_the_driver() {
        for (driver, want) in [
            (NetDriver::Bridge, "bridge"),
            (NetDriver::Macvlan, "macvlan"),
            (NetDriver::Ipvlan, "ipvlan"),
        ] {
            let f = fake_docker(vec![route(
                "/networks/create",
                r#"{"Id":"n1","Warning":""}"#,
            )]);
            assert_eq!(f.engine.create_network("my net", driver).await, Ok(()));
            let seen = f.seen();
            assert!(seen[0].starts_with("POST /"), "{}", seen[0]);
            let body = body_of(&seen[0]);
            assert_eq!(body["Name"], "my net");
            assert_eq!(body["Driver"], want);
        }
        let f = fake_docker(vec![]);
        assert!(
            f.engine
                .create_network("x1", NetDriver::Bridge)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn remove_volume_and_network_send_delete() {
        let f = fake_docker(vec![
            route("/volumes/data-1", ""),
            route("/networks/abc1", ""),
        ]);
        assert_eq!(f.engine.remove_volume("data-1").await, Ok(()));
        assert_eq!(f.engine.remove_network("abc1").await, Ok(()));
        let seen = f.seen();
        assert!(seen[0].starts_with("DELETE /") && seen[0].contains("/volumes/data-1"));
        assert!(seen[1].starts_with("DELETE /") && seen[1].contains("/networks/abc1"));
    }

    #[tokio::test]
    async fn prune_volumes_posts_and_returns_the_reclaimed_size() {
        let f = fake_docker(vec![route(
            "/volumes/prune",
            r#"{"VolumesDeleted":["a"],"SpaceReclaimed":7}"#,
        )]);
        assert_eq!(f.engine.prune_volumes().await, Ok(7));
        assert!(f.seen()[0].starts_with("POST /"));
        let f = fake_docker(vec![route("/volumes/prune", "{}")]);
        assert_eq!(f.engine.prune_volumes().await, Ok(0));
        let f = fake_docker(vec![]);
        assert!(f.engine.prune_volumes().await.is_err());
    }

    #[tokio::test]
    async fn prune_networks_counts_the_deleted_ones() {
        let f = fake_docker(vec![route(
            "/networks/prune",
            r#"{"NetworksDeleted":["a","b"]}"#,
        )]);
        assert_eq!(f.engine.prune_networks().await, Ok(2));
        assert!(f.seen()[0].starts_with("POST /"));
        let f = fake_docker(vec![route("/networks/prune", "{}")]);
        assert_eq!(f.engine.prune_networks().await, Ok(0));
        let f = fake_docker(vec![]);
        assert!(f.engine.prune_networks().await.is_err());
    }
}
