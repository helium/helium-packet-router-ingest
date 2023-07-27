use duration_string::DurationString;
use helium_crypto::PublicKeyBinary;
use helium_proto::{services::router::PacketRouterPacketUpV1, Region};
use hpr_http_rs::{
    gwmp::{self, app::UpdateAction as GwmpUpdateAction, settings::GwmpSettings},
    http_roaming::{
        self,
        app::UpdateAction as HttpUpdateAction,
        downlink::parse_http_payload,
        settings::{HttpSettings, NetworkSettings, ProtocolVersion, RoamingSettings},
        ul_token::make_join_token,
        HttpResponseResult,
    },
    uplink::{
        ingest::{GatewayID, GatewayTx, UplinkIngest},
        packet::{PacketUp, PacketUpTrait},
    },
    Result,
};

#[tokio::test]
async fn first_seen_packet_starts_timer() -> Result {
    let (tx, mut app) = make_http_app();

    // queue uplink received message
    let packet = join_req_packet_up();
    tx.uplink_receive(packet.clone()).await;

    // tick one message
    match http_roaming::app::handle_single_message(&mut app).await {
        HttpUpdateAction::StartTimerForNewPacket(hash) => {
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

    let _first_action = http_roaming::app::handle_single_message(&mut app).await;
    match http_roaming::app::handle_single_message(&mut app).await {
        HttpUpdateAction::Noop => (),
        _ => anyhow::bail!("expected second packet to result in nooop"),
    }
    assert_eq!(1, app.current_packet_count());
    assert_eq!(2, app.total_current_packet_count());

    Ok(())
}

#[tokio::test]
async fn http_gateway_connect_disconnect() -> Result {
    let (tx, mut app) = make_http_app();

    let (gw, _gw_rx) = tokio::sync::mpsc::channel(1);

    let gateway = GatewayID {
        b58: "one".to_string(),
        mac: "one".to_string(),
        region: Region::Us915,
        tx: GatewayTx(gw),
    };

    // Gateway connect
    tx.gateway_connect(gateway.clone()).await;
    tx.gateway_disconnect(gateway.b58).await;
    assert_eq!(0, app.gateway_count());

    match http_roaming::app::handle_single_message(&mut app).await {
        HttpUpdateAction::Noop => (),
        _ => anyhow::bail!("expected no action from gateway connect"),
    }
    assert_eq!(1, app.gateway_count());

    match http_roaming::app::handle_single_message(&mut app).await {
        HttpUpdateAction::Noop => (),
        _ => anyhow::bail!("expected no action from gateway disconnect"),
    }
    assert_eq!(0, app.gateway_count());

    Ok(())
}

#[tokio::test]
async fn gwmp_gateway_connect_disconnect() -> Result {
    let (tx, mut app) = make_gwmp_app();

    let (gw, _gw_rx) = tokio::sync::mpsc::channel(1);

    let gateway = GatewayID {
        b58: "13jnwnZYLDgw9Kd4zp33cyx4tBNQJ4jMoNvHTiFyvUkAgkhQUz9".to_string(),
        mac: "86213a0f50bce10e".to_string(),
        region: Region::Us915,
        tx: GatewayTx(gw),
    };

    // Gateway connect
    tx.gateway_connect(gateway.clone()).await;
    tx.gateway_disconnect(gateway.b58).await;
    assert_eq!(0, app.gateway_count());

    match gwmp::app::handle_single_message(&mut app).await {
        GwmpUpdateAction::Noop => (),
        _ => anyhow::bail!("expected no action from gateway connect"),
    }
    assert_eq!(1, app.gateway_count());

    match gwmp::app::handle_single_message(&mut app).await {
        GwmpUpdateAction::Noop => (),
        _ => anyhow::bail!("expected no action from gateway disconnect"),
    }
    assert_eq!(0, app.gateway_count());

    Ok(())
}

#[test]
fn gateway_vars() {
    let gw = vec![
        1, 104, 142, 151, 96, 143, 225, 209, 138, 18, 147, 96, 234, 13, 136, 61, 141, 21, 85, 67,
        15, 109, 219, 21, 144, 79, 162, 164, 169, 83, 242, 111, 165,
    ];
    let b58 = PublicKeyBinary::from(&gw[..]).to_string();
    let mac = hex::encode(xxhash_rust::xxh64::xxh64(&gw[1..], 0).to_be_bytes());

    println!("b58: {b58}");
    println!("mac: {mac}");
}

#[tokio::test]
async fn sending_non_existent_packet() -> Result {
    let (tx, mut app) = make_http_app();

    // Send message for packet hash never seen
    tx.uplink_send("fake-hash".into()).await;

    match http_roaming::app::handle_single_message(&mut app).await {
        HttpUpdateAction::Noop => (),
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
    http_roaming::app::handle_single_message(&mut app).await;
    assert_eq!(1, app.current_packet_count());

    match http_roaming::app::handle_single_message(&mut app).await {
        HttpUpdateAction::UplinkSend(_json_body) => (),
        a => anyhow::bail!("expected send uplink from known packet hash: {a:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn cleanup_unknown_packet() -> Result {
    let (tx, mut app) = make_http_app();

    // Send message for packet hash never seen
    tx.uplink_cleanup("fake-packet".into()).await;

    match http_roaming::app::handle_single_message(&mut app).await {
        HttpUpdateAction::Noop => (),
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
    http_roaming::app::handle_single_message(&mut app).await;
    assert_eq!(1, app.current_packet_count());

    match http_roaming::app::handle_single_message(&mut app).await {
        HttpUpdateAction::Noop => (),
        _ => anyhow::bail!("expected no action from known packet hash"),
    }
    assert_eq!(0, app.current_packet_count());

    Ok(())
}

#[tokio::test]
async fn send_downlink_to_known_gateway() -> Result {
    let (tx, mut app) = make_http_app();

    let (gw_tx, _gw_rx) = tokio::sync::mpsc::channel(1);

    let downlink = parse_http_payload(join_accept_payload(), &app.settings.roaming)
        .expect("result parseable")
        .expect("option contains downlink");

    let gw = GatewayID {
        b58: downlink.gateway_b58.clone(),
        mac: "mac".to_string(),
        region: Region::Us915,
        tx: GatewayTx(gw_tx),
    };

    // Gateway Connect
    tx.gateway_connect(gw).await;

    http_roaming::app::handle_single_message(&mut app).await;
    assert_eq!(1, app.gateway_count());

    tx.downlink(downlink).await;
    match http_roaming::app::handle_single_message(&mut app).await {
        HttpUpdateAction::DownlinkSend(_gw_chan, packet_down, http_response) => {
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

    let downlink = parse_http_payload(join_accept_payload(), &app.settings.roaming)
        .expect("result parseable")
        .expect("option contains downlink");

    assert_eq!(0, app.gateway_count());

    tx.downlink(downlink).await;
    match http_roaming::app::handle_single_message(&mut app).await {
        HttpUpdateAction::DownlinkError(http_response) => {
            assert_eq!(HttpResponseResult::XmitFailed, http_response.result);
        }
        x => anyhow::bail!("expected downlink action got: {x:?}"),
    }

    Ok(())
}

// helpers ========================================================================
fn join_req_packet_up() -> PacketUp {
    join_req_packet_up_from_gateway("")
}

fn join_req_packet_up_from_gateway(gw: &str) -> PacketUp {
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

fn make_http_app() -> (http_roaming::MsgSender, http_roaming::app::App) {
    let settings = default_http_roaming_settings();
    let (tx, rx) = http_roaming::MsgSender::new();
    let app = http_roaming::app::App::new(tx.clone(), rx, settings);
    (tx, app)
}

fn default_http_roaming_settings() -> HttpSettings {
    HttpSettings {
        roaming: RoamingSettings {
            protocol_version: ProtocolVersion::default(),
            helium_net_id: "C00053".to_string(),
            target_net_id: "000024".to_string(),
            sender_nsid: "sender-nsid".to_string(),
            receiver_nsid: "receiver-nsid".to_string(),
            dedup_window: DurationString::from_string("250ms".to_string()).unwrap(),
            authorization_header: Some("Auth header".to_string()),
        },
        network: NetworkSettings {
            lns_endpoint: "localhost:8080".to_string(),
            downlink_listen: "0.0.0.0:9000".parse().unwrap(),
            uplink_listen: "0.0.0.0:9001".parse().unwrap(),
        },
        cleanup_window: DurationString::from_string("10s".to_string()).unwrap(),
        metrics_listen: "0.0.0.0:9002".parse().unwrap(),
    }
}

fn make_gwmp_app() -> (gwmp::MsgSender, gwmp::app::App) {
    let settings = default_gwmp_settings();
    let (tx, rx) = gwmp::MsgSender::new();
    let app = gwmp::app::App::new(tx.clone(), rx, settings);
    (tx, app)
}

fn default_gwmp_settings() -> GwmpSettings {
    GwmpSettings {
        metrics_listen: "0.0.0.0:9002".parse().unwrap(),
        uplink_listen: "0.0.0.0:9001".parse().unwrap(),
        lns_endpoint: "127.0.0.1:1700".parse().unwrap(),
        region_port_mapping: Default::default(),
    }
}

fn join_accept_payload() -> serde_json::Value {
    let token = make_join_token("test-gateway".to_string(), 100, Region::Us915);
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
