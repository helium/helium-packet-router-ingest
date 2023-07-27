use super::{packet::to_packet_down, settings::GwmpSettings, Msg, MsgReceiver, MsgSender};
use crate::{
    uplink::{packet::PacketUp, Gateway, GatewayMac, GatewayTx},
    Result,
};
use semtech_udp::client_runtime::{self, ClientTx, DownlinkRequest, Event, UdpRuntime};
use std::{collections::HashMap, net::SocketAddr};
use tokio::sync::mpsc::Receiver;

pub struct App {
    pub settings: GwmpSettings,
    message_tx: MsgSender,
    message_rx: MsgReceiver,
    forward_chans: HashMap<GatewayMac, (ClientTx, GatewayTx, triggered::Trigger)>,
}

impl App {
    pub fn new(message_tx: MsgSender, message_rx: MsgReceiver, settings: GwmpSettings) -> Self {
        Self {
            settings,
            message_tx,
            message_rx,
            forward_chans: Default::default(),
        }
    }

    pub fn gateway_count(&self) -> usize {
        self.forward_chans.len()
    }
}

pub struct Client {
    pub mac: GatewayMac,
    pub udp_runtime: UdpRuntime,
    pub shutdown_listener: triggered::Listener,
    pub downlink_receiver: Receiver<Event>,
}

#[derive()]
pub enum UpdateAction {
    Noop,
    NewClient(Client),
    Uplink(ClientTx, PacketUp),
    Downlink(GatewayTx, DownlinkRequest),
}

pub async fn start(
    message_tx: MsgSender,
    message_rx: MsgReceiver,
    settings: GwmpSettings,
) -> Result {
    let mut app = App::new(message_tx, message_rx, settings);
    loop {
        let action = handle_single_message(&mut app).await;
        handle_update_action(&app, action).await;
    }
}

pub async fn handle_single_message(app: &mut App) -> UpdateAction {
    let message = app.message_rx.recv().await.unwrap();
    handle_message(app, message).await
}

async fn handle_message(app: &mut App, msg: Msg) -> UpdateAction {
    match msg {
        Msg::GatewayConnect(gw) => {
            let (uplink_forwarder, downlink_receiver, udp_runtime) =
                client_runtime::UdpRuntime::new(
                    gw.mac.clone().into(),
                    endpoint_for_gateway(app, &gw),
                )
                .await
                .expect("create udp runtime");

            let (shutdown_trigger, shutdown_listener) = triggered::trigger();
            app.forward_chans
                .insert(gw.mac.clone(), (uplink_forwarder, gw.tx, shutdown_trigger));

            return UpdateAction::NewClient(Client {
                mac: gw.mac,
                udp_runtime,
                shutdown_listener,
                downlink_receiver,
            });
        }
        Msg::GatewayDisconnect(gw_mac) => {
            tracing::info!(?gw_mac, "gateway disconnected");
            match app.forward_chans.get(&gw_mac) {
                Some((_, _, shutdown)) => shutdown.trigger(),
                None => {
                    tracing::warn!(
                        ?gw_mac,
                        "something went wrong, gateway already disconnected"
                    );
                }
            }
            app.forward_chans.remove(&gw_mac);
        }
        Msg::Uplink(packet_up) => {
            let gw_mac = packet_up.gateway_mac();
            match app.forward_chans.get(&gw_mac) {
                Some((chan, _, _)) => {
                    tracing::info!(?gw_mac, "uplink");
                    return UpdateAction::Uplink(chan.clone(), packet_up);
                }
                None => todo!(),
            }
        }
        Msg::Downlink(gw_id, packet_down) => match app.forward_chans.get(&gw_id) {
            Some((_, chan, _)) => {
                tracing::info!(?gw_id, "downlink");
                return UpdateAction::Downlink(chan.clone(), packet_down);
            }
            None => todo!(),
        },
    }
    UpdateAction::Noop
}

pub async fn handle_update_action(app: &App, action: UpdateAction) {
    match action {
        UpdateAction::Noop => {}
        UpdateAction::Uplink(chan, packet_up) => {
            chan.send(packet_up.into()).await.expect("uplink forwarded");
        }
        UpdateAction::Downlink(chan, downlink_request) => {
            chan.send_downlink(to_packet_down(downlink_request.txpk()))
                .await;
        }
        UpdateAction::NewClient(client) => {
            tokio::spawn(client.udp_runtime.run(client.shutdown_listener));
            let message_tx = app.message_tx.clone();
            tokio::spawn(async move {
                let mut downlink_receiver = client.downlink_receiver;
                while let Some(x) = downlink_receiver.recv().await {
                    match x {
                        client_runtime::Event::Reconnected => todo!("reconnected"),
                        client_runtime::Event::LostConnection => {
                            todo!("udp connection lost")
                        }
                        client_runtime::Event::DownlinkRequest(downlink) => {
                            tracing::info!(?client.mac, "sending downlink");
                            message_tx.send_downlink(client.mac.clone(), downlink).await
                        }
                        client_runtime::Event::UnableToParseUdpFrame(_, _) => {
                            todo!("unable to parse udp frame")
                        }
                    }
                }
            });
        }
    }
}

fn endpoint_for_gateway(app: &App, gw: &Gateway) -> SocketAddr {
    let mut endpoint = app.settings.lns_endpoint;
    if let Some(&port) = app.settings.region_port_mapping.get(&gw.region) {
        endpoint.set_port(port);
    }
    endpoint
}
