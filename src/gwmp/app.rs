use super::{packet::to_packet_down, settings::GwmpSettings, Msg, MsgReceiver, MsgSender};
use crate::{
    uplink::{packet::PacketUp, Gateway, GatewayMac},
    Result,
};
use semtech_udp::client_runtime::{self, ClientTx, DownlinkRequest, Event, UdpRuntime};
use std::{collections::HashMap, fmt::Debug, net::SocketAddr};
use tokio::sync::mpsc::Receiver;
use tracing::Instrument;

pub struct App {
    pub settings: GwmpSettings,
    message_tx: MsgSender,
    message_rx: MsgReceiver,
    forward_chans: HashMap<GatewayMac, UDPForwarder>,
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

    fn add_gateway(
        &mut self,
        gateway: Gateway,
        client_tx: ClientTx,
        shutdown_trigger: triggered::Trigger,
    ) {
        self.forward_chans.insert(
            gateway.mac,
            UDPForwarder {
                tx: client_tx,
                shutdown_trigger,
            },
        );
    }
}

#[derive(Debug, Clone)]
pub struct UDPForwarder {
    tx: ClientTx,
    shutdown_trigger: triggered::Trigger,
}

pub enum UpdateAction {
    Noop,
    NewForwarder {
        gateway: Gateway,
        udp_runtime: UdpRuntime,
        shutdown_listener: triggered::Listener,
        downlink_receiver: Receiver<Event>,
    },
    Uplink(UDPForwarder, PacketUp),
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
            app.add_gateway(gateway.clone(), uplink_forwarder, shutdown_trigger);

            return UpdateAction::NewForwarder {
                gateway,
                udp_runtime,
                shutdown_listener,
                downlink_receiver,
            };
        }
        Msg::GatewayDisconnect(gw) => {
            tracing::info!(mac = ?gw.mac, "gateway disconnected");
            match app.forward_chans.remove(&gw.mac) {
                Some(gwmp_gw) => gwmp_gw.shutdown_trigger.trigger(),
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
                    return UpdateAction::Uplink(gwmp_gw.clone(), packet_up);
                }
                None => tracing::warn!(
                    mac = ?gw_mac,
                    b58 = ?packet_up.gateway_b58(),
                    "packet received for unknown gateway"
                ),
            }
        }
        Msg::Downlink(gateway, packet_down) => {
            tracing::info!(?gateway, "downlink");
            return UpdateAction::Downlink(gateway, packet_down);
        }
    }
    UpdateAction::Noop
}

pub async fn handle_update_action(app: &App, action: UpdateAction) {
    match action {
        UpdateAction::Noop => {}
        UpdateAction::Uplink(gwmp_gw, packet_up) => {
            gwmp_gw
                .tx
                .send(packet_up.into())
                .await
                .expect("uplink forwarded");
        }
        UpdateAction::Downlink(gateway, downlink_request) => {
            gateway
                .tx
                .send_downlink(to_packet_down(downlink_request.txpk()))
                .await;
        }
        UpdateAction::NewForwarder {
            gateway,
            udp_runtime,
            shutdown_listener,
            mut downlink_receiver,
        } => {
            let udp_runtime_span = tracing::info_span!("udp runtime", mac = ?gateway.mac);
            tokio::spawn(
                udp_runtime
                    .run(shutdown_listener)
                    .instrument(udp_runtime_span),
            );

            let downlink_span = tracing::info_span!("downlink listener", mac = ?gateway.mac);
            let message_tx = app.message_tx.clone();
            tokio::spawn(
                async move {
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
                                message_tx.send_downlink(gateway.clone(), downlink).await
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

// UDPRuntime does not derive Debug, so we omit it here
impl std::fmt::Debug for UpdateAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Noop => write!(f, "Noop"),
            Self::NewForwarder {
                gateway,
                udp_runtime: _,
                shutdown_listener,
                downlink_receiver,
            } => f
                .debug_struct("NewForwarder")
                .field("gateway", gateway)
                .field("shutdown_listener", shutdown_listener)
                .field("downlink_receiver", downlink_receiver)
                .finish(),
            Self::Uplink(arg0, arg1) => f.debug_tuple("Uplink").field(arg0).field(arg1).finish(),
            Self::Downlink(arg0, arg1) => {
                f.debug_tuple("Downlink").field(arg0).field(arg1).finish()
            }
        }
    }
}
