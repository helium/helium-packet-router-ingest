use duration_string::DurationString;
use helium_proto::services::router::PacketRouterPacketUpV1;
use hpr_http_rs::{
    http_roaming::{
        app::{self, UpdateAction},
        downlink::parse_http_payload,
        settings::{HttpSettings, ProtocolVersion, RoamingSettings},
        ul_token::make_join_token,
        HttpResponseResult, MsgSender,
    },
    region::Region,
    uplink::{ingest::UplinkIngest, packet::PacketUp, Gateway, GatewayB58, GatewayMac, GatewayTx},
    Result,
};
use tokio::sync::mpsc::{error::TryRecvError, Receiver};

#[tokio::test]
async fn first_seen_packet_starts_timer() -> Result {
    let (tx, mut app) = make_http_app();

    // queue uplink received message
    let packet = join_req_packet_up();
    tx.uplink_receive(packet.clone()).await;

    // tick one message
    match app::handle_single_message(&mut app).await {
        UpdateAction::StartTimerForNewPacket(hash) => {
            assert_eq!(hash, packet.hash())
        }
        _ => anyhow::bail!("expected start timer for new packet message"),
    };

    Ok(())
}

#[tokio::test]
async fn stores_duplicate_packets() -> Result {
    let (tx, mut app) = make_http_app();

    let packet1 = join_req_packet_up_from_gateway("one");
    let packet2 = join_req_packet_up_from_gateway("two");
    tx.uplink_receive(packet1.clone()).await;
    tx.uplink_receive(packet2.clone()).await;

    let _first_action = app::handle_single_message(&mut app).await;
    match app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        _ => anyhow::bail!("expected second packet to result in nooop"),
    }
    assert_eq!(1, app.current_packet_count());
    assert_eq!(2, app.total_current_packet_count());

    Ok(())
}

#[tokio::test]
async fn http_gateway_connect_disconnect() -> Result {
    let (tx, mut app) = make_http_app();

    let (gw, mut gw_rx) = tokio::sync::mpsc::channel(1);

    let gateway = Gateway {
        b58: GatewayB58("one".to_string()),
        mac: GatewayMac("one".to_string()),
        region: Region::Us915,
        tx: GatewayTx(gw),
    };

    // Gateway connect
    tx.gateway_connect(gateway.clone()).await;
    tx.gateway_disconnect(gateway).await;
    assert_eq!(0, app.gateway_count());

    match app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        _ => anyhow::bail!("expected no action from gateway connect"),
    }
    assert_eq!(1, app.gateway_count());
    assert!(channel_is_open(&mut gw_rx)?);

    match app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        _ => anyhow::bail!("expected no action from gateway disconnect"),
    }
    assert_eq!(0, app.gateway_count());
    assert!(channel_is_closed(&mut gw_rx)?);

    Ok(())
}

#[tokio::test]
async fn sending_non_existent_packet() -> Result {
    let (tx, mut app) = make_http_app();

    // Send message for packet hash never seen
    tx.uplink_send("fake-hash".into()).await;

    match app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        _ => anyhow::bail!("expected no action from unknown packet hash"),
    }

    Ok(())
}

#[tokio::test]
async fn sending_packet() -> Result {
    let (tx, mut app) = make_http_app();

    // Send message for packet hash never seen
    let packet = join_req_packet_up_from_gateway("one");
    tx.uplink_receive(packet.clone()).await;
    tx.uplink_send(packet.hash()).await;

    // ingest packet
    app::handle_single_message(&mut app).await;
    assert_eq!(1, app.current_packet_count());

    match app::handle_single_message(&mut app).await {
        UpdateAction::UplinkSend(_json_body) => (),
        a => anyhow::bail!("expected send uplink from known packet hash: {a:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn cleanup_unknown_packet() -> Result {
    let (tx, mut app) = make_http_app();

    // Send message for packet hash never seen
    tx.uplink_cleanup("fake-packet".into()).await;

    match app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        _ => anyhow::bail!("expected no action from unknown packet hash"),
    }

    Ok(())
}

#[tokio::test]
async fn cleanup_packet() -> Result {
    let (tx, mut app) = make_http_app();

    // Send message for packet hash never seen
    let packet = join_req_packet_up_from_gateway("one");
    tx.uplink_receive(packet.clone()).await;
    tx.uplink_cleanup(packet.hash()).await;

    // ingest packet
    app::handle_single_message(&mut app).await;
    assert_eq!(1, app.current_packet_count());

    match app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        _ => anyhow::bail!("expected no action from known packet hash"),
    }
    assert_eq!(0, app.current_packet_count());

    Ok(())
}

#[tokio::test]
async fn send_downlink_to_known_gateway() -> Result {
    let (tx, mut app) = make_http_app();

    let (gw_tx, _gw_rx) = tokio::sync::mpsc::channel(1);

    let downlink = parse_http_payload(join_accept_json(), &app.settings.roaming)
        .expect("result parseable")
        .expect("option contains downlink");

    let gw = Gateway {
        b58: downlink.gateway_b58.clone(),
        mac: downlink.gateway_b58.clone().into(),
        region: Region::Us915,
        tx: GatewayTx(gw_tx),
    };

    // Gateway Connect
    tx.gateway_connect(gw).await;

    app::handle_single_message(&mut app).await;
    assert_eq!(1, app.gateway_count());

    tx.downlink(downlink).await;
    match app::handle_single_message(&mut app).await {
        UpdateAction::DownlinkSend(_gw_chan, packet_down, http_response) => {
            assert_eq!(HttpResponseResult::Success, http_response.result);
            assert_eq!(packet_down, packet_down);
        }
        x => anyhow::bail!("expected downlink action got: {x:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn downlink_to_unknown_gateway_responds_error() -> Result {
    let (tx, mut app) = make_http_app();

    let downlink = parse_http_payload(join_accept_json(), &app.settings.roaming)
        .expect("result parseable")
        .expect("option contains downlink");

    assert_eq!(0, app.gateway_count());

    tx.downlink(downlink).await;
    match app::handle_single_message(&mut app).await {
        UpdateAction::DownlinkError(http_response) => {
            assert_eq!(HttpResponseResult::XmitFailed, http_response.result);
        }
        x => anyhow::bail!("expected downlink action got: {x:?}"),
    }

    Ok(())
}

// helpers ========================================================================
pub fn join_req_packet_up() -> PacketUp {
    join_req_packet_up_from_gateway("")
}

pub fn join_req_packet_up_from_gateway(gw: &str) -> PacketUp {
    let packet = PacketRouterPacketUpV1 {
        payload: vec![
            0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 196, 160, 173, 225, 146, 91,
        ],
        timestamp: 0,
        rssi: 0,
        frequency: 0,
        datarate: 2,
        snr: 0.0,
        region: 0,
        hold_time: 0,
        gateway: gw.into(),
        signature: vec![],
    };
    PacketUp::new(packet, 0)
}

fn make_http_app() -> (MsgSender, app::App) {
    let settings = default_http_roaming_settings();
    let (tx, rx) = MsgSender::new();
    let app = app::App::new(tx.clone(), rx, settings);
    (tx, app)
}

fn default_http_roaming_settings() -> HttpSettings {
    HttpSettings {
        roaming: RoamingSettings {
            protocol_version: ProtocolVersion::default(),
            sender_net_id: "C00053".to_string(),
            receiver_net_id: "000024".to_string(),
            sender_nsid: "sender-nsid".to_string(),
            receiver_nsid: "receiver-nsid".to_string(),
            dedup_window: DurationString::from_string("250ms".to_string()).unwrap(),
            authorization_header: Some("Auth header".to_string()),
        },
        cleanup_window: DurationString::from_string("10s".to_string()).unwrap(),
        metrics_listen: "0.0.0.0:9002".parse().unwrap(),
        lns_endpoint: "localhost:8080".to_string(),
        downlink_listen: "0.0.0.0:9000".parse().unwrap(),
        uplink_listen: "0.0.0.0:9001".parse().unwrap(),
    }
}

fn join_accept_json() -> serde_json::Value {
    let token = make_join_token(
        GatewayB58("13jnwnZYLDgw9Kd4zp33cyx4tBNQJ4jMoNvHTiFyvUkAgkhQUz9".to_string()),
        100,
        Region::Us915,
    );
    serde_json::json!({
        "ProtocolVersion": "1.1",
        "SenderID": "000024",
        "ReceiverID": "c00053",
        "TransactionID":  193858937,
        "MessageType": "PRStartAns",
        "Result": {"ResultCode": "Success"},
        "PHYPayload": "209c7848d681b589da4b8e5544460a693383bb7e5d47b849ef0d290bafb20872ae",
        "DevEUI": "0000000000000003",
        "Lifetime": null,
        "FNwkSIntKey": null,
        "NwkSKey": {
            "KEKLabel": "",
            "AESKey": "0e2baf26327308e63afe62be15edea6a"
        },
        "FCntUp": 0,
        "ServiceProfile": null,
        "DLMetaData": {
            "DevEUI": "0000000000000003",
            "FPort": null,
            "FCntDown": null,
            "Confirmed": false,
            "DLFreq1": 925.1,
            "DLFreq2": 923.3,
            "RXDelay1": 3,
            "ClassMode": "A",
            "DataRate1": 10,
            "DataRate2": 8,
            "FNSULToken": token,
            "GWInfo": [{"FineRecvTime": null,"RSSI": null,"SNR": null,"Lat": null,"Lon": null,"DLAllowed": null}],
          "HiPriorityFlag": false
        },
        "DevAddr": "48000037"
    })
}

fn channel_is_open<T: std::fmt::Debug>(chan: &mut Receiver<T>) -> Result<bool> {
    match chan.try_recv() {
        Err(TryRecvError::Empty) => Ok(true),
        val => anyhow::bail!("expecty empty open channel, got: {val:?}"),
    }
}

fn channel_is_closed<T: std::fmt::Debug>(chan: &mut Receiver<T>) -> Result<bool> {
    match chan.try_recv() {
        Err(TryRecvError::Disconnected) => Ok(true),
        val => anyhow::bail!("expected closed channel, got: {val:?}"),
    }
}
