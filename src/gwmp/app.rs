use super::{packet::to_packet_down, settings::GwmpSettings, Msg, MsgReceiver, MsgSender};
use crate::{
    uplink::{packet::PacketUp, Gateway, GatewayMac},
    Result,
};
use semtech_udp::client_runtime::{self, ClientTx, DownlinkRequest, Event, UdpRuntime};
use std::{collections::HashMap, fmt::Debug, net::SocketAddr};
use tokio::sync::mpsc::Receiver;
use tracing::Instrument;

struct GwmpGateway {
    client_tx: ClientTx,
    gateway: Gateway,
    shutdown_trigger: triggered::Trigger,
}

impl GwmpGateway {
    fn new(client_tx: ClientTx, gateway: Gateway, shutdown_trigger: triggered::Trigger) -> Self {
        Self {
            client_tx,
            gateway,
            shutdown_trigger,
        }
    }
    fn shutdown(&self) {
        self.shutdown_trigger.trigger();
    }
}

type GwmpGateways = HashMap<GatewayMac, GwmpGateway>;

pub struct App {
    pub settings: GwmpSettings,
    message_tx: MsgSender,
    message_rx: MsgReceiver,
    forward_chans: GwmpGateways,
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
    pub gateway: Gateway,
    pub udp_runtime: UdpRuntime,
    pub shutdown_listener: triggered::Listener,
    pub downlink_receiver: Receiver<Event>,
}

impl Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("gateway", &self.gateway)
            .finish()
    }
}

#[derive(Debug)]
pub enum UpdateAction {
    Noop,
    NewClient(Client),
    Uplink(ClientTx, PacketUp),
    Downlink(Gateway, DownlinkRequest),
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
        Msg::GatewayConnect(gateway) => {
            let (uplink_forwarder, downlink_receiver, udp_runtime) =
                client_runtime::UdpRuntime::new(
                    gateway.mac.clone().into(),
                    endpoint_for_gateway_region(app, &gateway),
                )
                .await
                .expect("create udp runtime");

            let (shutdown_trigger, shutdown_listener) = triggered::trigger();
            app.forward_chans.insert(
                gateway.mac.clone(),
                GwmpGateway::new(uplink_forwarder, gateway.clone(), shutdown_trigger),
            );

            return UpdateAction::NewClient(Client {
                gateway,
                udp_runtime,
                shutdown_listener,
                downlink_receiver,
            });
        }
        Msg::GatewayDisconnect(gw) => {
            tracing::info!(mac = ?gw.mac, "gateway disconnected");
            match app.forward_chans.remove(&gw.mac) {
                Some(gwmp_gw) => gwmp_gw.shutdown(),
                None => {
                    tracing::warn!(
                        ?gw.mac,
                        "something went wrong, gateway already disconnected"
                    );
                }
            }
        }
        Msg::Uplink(packet_up) => {
            let gw_mac = packet_up.gateway_mac();
            match app.forward_chans.get(&gw_mac) {
                Some(gwmp_gw) => {
                    tracing::info!(?gw_mac, "uplink");
                    return UpdateAction::Uplink(gwmp_gw.client_tx.clone(), packet_up);
                }
                None => tracing::warn!(
                    mac = ?gw_mac,
                    b58 = ?packet_up.gateway_b58(),
                    "packet received for unknown gateway"
                ),
            }
        }
        Msg::Downlink(gateway, packet_down) => match app.forward_chans.get(&gateway.mac) {
            Some(gwmp_gw) => {
                tracing::info!(?gateway, "downlink");
                return UpdateAction::Downlink(gwmp_gw.gateway.clone(), packet_down);
            }
            None => tracing::warn!(?gateway, "downlink received for unknown gateway"),
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
        UpdateAction::Downlink(gateway, downlink_request) => {
            gateway
                .tx
                .send_downlink(to_packet_down(downlink_request.txpk()))
                .await;
        }
        UpdateAction::NewClient(client) => {
            let udp_runtime_span = tracing::info_span!("udp runtime", mac = ?client.gateway.mac);
            tokio::spawn(
                client
                    .udp_runtime
                    .run(client.shutdown_listener)
                    .instrument(udp_runtime_span),
            );

            let downlink_span = tracing::info_span!("downlink listener", mac = ?client.gateway.mac);
            let message_tx = app.message_tx.clone();
            tokio::spawn(
                async move {
                    let mut downlink_receiver = client.downlink_receiver;
                    while let Some(x) = downlink_receiver.recv().await {
                        match x {
                            client_runtime::Event::Reconnected => {
                                tracing::info!("udp client runtime was reconnected, doing nothing");
                            }
                            client_runtime::Event::LostConnection => {
                                tracing::info!("udp client runtime lost connection, doing nothing");
                            }
                            client_runtime::Event::DownlinkRequest(downlink) => {
                                tracing::info!("sending downlink");
                                message_tx
                                    .send_downlink(client.gateway.clone(), downlink)
                                    .await
                            }
                            client_runtime::Event::UnableToParseUdpFrame(parse_err, data) => {
                                tracing::warn!(
                                    ?parse_err,
                                    ?data,
                                    "udp client runtime unable to parse frame"
                                );
                            }
                        }
                    }
                }
                .instrument(downlink_span),
            );
        }
    }
}

fn endpoint_for_gateway_region(app: &App, gw: &Gateway) -> SocketAddr {
    let mut endpoint = app.settings.lns_endpoint;
    if let Some(&port) = app.settings.region_port_mapping.get(&gw.region) {
        endpoint.set_port(port);
    }
    endpoint
}
