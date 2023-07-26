use super::{packet::to_packet_down, settings::GwmpSettings, Msg, MsgReceiver, MsgSender};
use crate::{
    uplink::{
        ingest::{GatewayID, GatewayTx},
        packet::{PacketUp, PacketUpTrait},
    },
    Result,
};
use semtech_udp::{
    client_runtime::{ClientTx, DownlinkRequest},
    MacAddress,
};
use std::{collections::HashMap, str::FromStr};

pub struct App {
    pub settings: GwmpSettings,
    message_tx: MsgSender,
    message_rx: MsgReceiver,
    forward_chans: HashMap<String, (ClientTx, GatewayTx, triggered::Trigger)>,
}

impl App {
    fn new(message_tx: MsgSender, message_rx: MsgReceiver, settings: GwmpSettings) -> Self {
        Self {
            settings,
            message_tx,
            message_rx,
            forward_chans: Default::default(),
        }
    }
}

#[derive(Debug)]
pub enum UpdateAction {
    Noop,
    ForwardUplink(ClientTx, PacketUp),
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
            tracing::info!(gw.b58, "gateway connected");
            new_client(app, gw).await;
        }
        Msg::GatewayDisconnect(gw_b58) => {
            tracing::info!(gw_b58, "gateway disconnected");
            match app.forward_chans.get(&gw_b58) {
                Some((_, _, shutdown)) => shutdown.trigger(),
                None => todo!(),
            }
            app.forward_chans.remove(&gw_b58);
        }
        Msg::Uplink(packet_up) => {
            let gateway_mac = packet_up.gateway_mac_str();
            match app.forward_chans.get(&gateway_mac) {
                Some((chan, _, _)) => {
                    tracing::info!(gateway_mac, "uplink");
                    return UpdateAction::ForwardUplink(chan.clone(), packet_up);
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

pub async fn handle_update_action(_app: &App, action: UpdateAction) {
    match action {
        UpdateAction::Noop => {}
        UpdateAction::ForwardUplink(chan, packet_up) => {
            chan.send(packet_up.into()).await.expect("uplink forwarded");
        }
        UpdateAction::Downlink(chan, downlink_request) => {
            chan.send_downlink(to_packet_down(downlink_request.txpk()))
                .await;
        }
    }
}

pub async fn new_client(app: &mut App, gw: GatewayID) {
    tracing::debug!(?app.settings.lns_endpoint, gw.b58, "starting a new udp client for gateway");
    let mac_address = MacAddress::from_str(&gw.mac).expect("mac address from b58");

    let mut endpoint = app.settings.lns_endpoint.clone();
    if let Some(&port) = app.settings.region_port_mapping.get(&gw.region) {
        endpoint.set_port(port);
    }

    // UplinkForwarder: Give this channel packets to send over UDP
    // DownlinkReceiver: Channel coming downstream from UDP
    let (uplink_forwarder, mut downlink_receiver, udp_runtime) =
        semtech_udp::client_runtime::UdpRuntime::new(mac_address, endpoint)
            .await
            .expect("create udp runtime");
    let (shutdown_trigger, shutdown_listener) = triggered::trigger();
    app.forward_chans.insert(
        gw.mac.to_string(),
        (uplink_forwarder, gw.tx, shutdown_trigger),
    );

    // TODO: Move this out of here
    // Start the UDP runtime
    tokio::spawn(udp_runtime.run(shutdown_listener));
    // Start listening for downlinks
    let message_tx = app.message_tx.clone();
    tokio::spawn(async move {
        while let Some(x) = downlink_receiver.recv().await {
            match x {
                semtech_udp::client_runtime::Event::Reconnected => todo!("reconnected"),
                semtech_udp::client_runtime::Event::LostConnection => {
                    todo!("udp connection lost")
                }
                semtech_udp::client_runtime::Event::DownlinkRequest(downlink) => {
                    tracing::info!(gw.mac, "sending downlink");
                    message_tx.send_downlink(gw.mac.to_string(), downlink).await
                }
                semtech_udp::client_runtime::Event::UnableToParseUdpFrame(_, _) => {
                    todo!("unable to parse udp frame")
                }
            }
        }
    });
}
