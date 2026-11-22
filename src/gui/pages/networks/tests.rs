use super::super::test_util::{harness, requests, state_with};
use super::*;
use crate::gui::state::Page;
use crate::snapshot::DockerSummary;
use egui_kittest::kittest::{NodeT, Queryable};

fn network(id: &str, name: &str, builtin: bool) -> NetworkRow {
    NetworkRow {
        id: id.into(),
        name: name.into(),
        driver: "bridge".into(),
        builtin,
    }
}

fn networks_state(networks: Vec<NetworkRow>) -> GuiState {
    state_with(
        DockerSummary {
            networks,
            ..Default::default()
        },
        Page::Networks,
    )
}

#[test]
fn builtin_network_has_no_delete_button() {
    let rows = vec![
        network("b1b1b1b1b1b1b1", "bridge", true),
        network("c2c2c2c2c2c2c2", "app-net", false),
    ];
    let (h, _ends) = harness(Lang::En, networks_state(rows));
    assert!(h.query_by_label("Delete bridge").is_none());
    h.get_by_label("Delete app-net");
    // 削除できない理由を文字で出す。
    h.get_by_label("built-in");
}

#[test]
fn network_delete_sends_the_id_even_when_the_name_has_a_space() {
    let rows = vec![network("abc123abc123abc", "my net", false)];
    let (mut h, mut ends) = harness(Lang::En, networks_state(rows));
    h.get_by_label("Delete my net").click();
    h.run_steps(4);
    assert_eq!(
        h.state().0.confirm.clone().unwrap().message,
        "Delete network my net?"
    );
    assert!(requests(&mut ends).is_empty());
    h.get_by_label("Delete").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::RemoveNetwork {
            profile: "dev".into(),
            id: "abc123abc123abc".into()
        }]
    );
    assert!(h.state().0.busy_ops.contains("network:abc123abc123abc"));
}

#[test]
fn cancelling_the_network_confirmation_sends_nothing() {
    let rows = vec![network("abc123abc123abc", "app-net", false)];
    let (mut h, mut ends) = harness(Lang::En, networks_state(rows));
    h.get_by_label("Delete app-net").click();
    h.run_steps(4);
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    assert!(requests(&mut ends).is_empty());
}

#[test]
fn network_cleanup_confirms_then_prunes() {
    let (mut h, mut ends) = harness(Lang::En, networks_state(vec![]));
    h.get_by_label("Clean up…").click();
    h.run_steps(4);
    assert_eq!(
        h.state().0.confirm.clone().unwrap().message,
        "Delete all unused networks?"
    );
    h.get_by_label("Delete").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::PruneNetworks {
            profile: "dev".into()
        }]
    );
    assert!(h.state().0.busy_ops.contains("prune-networks:dev"));
    assert!(h.get_by_label("Clean up…").accesskit_node().is_disabled());
}

#[test]
fn rows_show_driver_and_short_id() {
    let rows = vec![network("abc123abc123abc", "app-net", false)];
    let (h, _ends) = harness(Lang::En, networks_state(rows));
    h.get_by_label("bridge");
    h.get_by_label("abc123abc123");
}

#[test]
fn empty_list_says_so() {
    let (h, _ends) = harness(Lang::Ja, networks_state(vec![]));
    h.get_by_label("ネットワークはありません");
}

#[test]
fn create_form_sends_name_and_driver_even_with_a_space_in_the_name() {
    let (mut h, mut ends) = harness(Lang::En, networks_state(vec![]));
    assert!(h.get_by_label("Create").accesskit_node().is_disabled());
    h.state_mut().0.networks.name = "wk e2e net".into();
    h.state_mut().0.networks.driver = NetDriver::Macvlan;
    h.run_steps(4);
    h.get_by_label("Create").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::CreateNetwork {
            profile: "dev".into(),
            name: "wk e2e net".into(),
            driver: NetDriver::Macvlan
        }]
    );
    assert!(h.state().0.busy_ops.contains("create-network:dev"));
}

#[test]
fn a_blank_name_cannot_be_created() {
    let (mut h, _ends) = harness(Lang::En, networks_state(vec![]));
    h.state_mut().0.networks.name = "   ".into();
    h.run_steps(4);
    assert!(h.get_by_label("Create").accesskit_node().is_disabled());
}

#[test]
fn the_default_driver_is_bridge() {
    assert_eq!(View::default().driver, NetDriver::Bridge);
}

#[test]
fn the_driver_can_be_chosen_from_the_list() {
    let (mut h, mut ends) = harness(Lang::En, networks_state(vec![]));
    h.get_by_label("Driver").click();
    h.run_steps(4);
    h.get_by_label("ipvlan").click();
    h.run_steps(4);
    assert_eq!(h.state().0.networks.driver, NetDriver::Ipvlan);
    h.state_mut().0.networks.name = "net-1".into();
    h.run_steps(4);
    h.get_by_label("Create").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::CreateNetwork {
            profile: "dev".into(),
            name: "net-1".into(),
            driver: NetDriver::Ipvlan
        }]
    );
}

#[test]
fn network_inspect_asks_by_id_and_builtin_networks_can_be_inspected() {
    let rows = vec![
        network("abc123abc123abc", "my net", false),
        network("b1b1b1b1b1b1b1", "bridge", true),
    ];
    let (mut h, mut ends) = harness(Lang::En, networks_state(rows));
    h.get_by_label("Inspect my net").click();
    h.run_steps(4);
    h.get_by_label("Inspect bridge").click();
    h.run_steps(4);
    let targets: Vec<String> = requests(&mut ends)
        .into_iter()
        .map(|r| match r {
            Request::InspectResource { target, .. } => target,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(targets, ["abc123abc123abc", "b1b1b1b1b1b1b1"]);
}
