//! ボリューム・ネットワークの操作と、全体の整理。

use super::{reclaimed, to_json};
use crate::docker::{DockerError, Engine, check_name};
use bollard::models::{NetworkCreateRequest, VolumeCreateRequest};
use bollard::query_parameters::{
    InspectNetworkOptions, PruneContainersOptions, PruneNetworksOptions, PruneVolumesOptions,
    RemoveVolumeOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetDriver {
    Bridge,
    Macvlan,
    Ipvlan,
}

impl NetDriver {
    fn as_str(self) -> &'static str {
        match self {
            NetDriver::Bridge => "bridge",
            NetDriver::Macvlan => "macvlan",
            NetDriver::Ipvlan => "ipvlan",
        }
    }
}

impl Engine {
    pub async fn create_volume(&self, name: &str) -> Result<(), DockerError> {
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

    /// 停止中のコンテナ、使われていないネットワーク、dangling イメージを消す。ボリュームは消さない。
    pub async fn system_prune(&self) -> Result<u64, DockerError> {
        let containers = self
            .docker
            .prune_containers(None::<PruneContainersOptions>)
            .await?;
        self.prune_networks().await?;
        let images = self.prune_images(true).await?;
        Ok(reclaimed(containers.space_reclaimed) + images)
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
}
