use duration_string::DurationString;
use helium_proto::services::router::PacketRouterPacketUpV1;
use hpr_http_rs::{
    app::{self, MsgSender, UpdateAction},
    settings::{NetworkSettings, RoamingSettings, Settings},
    uplink::{PacketUp, PacketUpTrait},
    uplink_ingest::GatewayTx,
    Result,
};

#[test]
fn app_can_be_constructed() -> Result {
    let settings = default_settings();

    let (tx, rx) = MsgSender::new();
    let _app = app::App::new(tx, rx, settings);

    Ok(())
}

#[tokio::test]
async fn first_seen_packet_starts_timer() -> Result {
    let (tx, mut app) = make_app();

    // queue uplink received message
    let packet = packet_up();
    tx.uplink_receive(packet.clone()).await;

    // tick one message
    match app::handle_single_message(&mut app).await {
        UpdateAction::StartTimerForNewPacket(hash) => {
            assert_eq!(hash, packet.hash())
        }
        _ => panic!("expected start timer for new packet message"),
    };

    Ok(())
}

#[tokio::test]
async fn stores_duplicate_packets() -> Result {
    let (tx, mut app) = make_app();

    let packet1 = packet_up_from_gateway("one");
    let packet2 = packet_up_from_gateway("two");
    tx.uplink_receive(packet1.clone()).await;
    tx.uplink_receive(packet2.clone()).await;

    let _first_action = app::handle_single_message(&mut app).await;
    match app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        _ => panic!("expected second packet to result in nooop"),
    }

    Ok(())
}

#[tokio::test]
async fn gateway_connect_disconnect() -> Result {
    let (tx, mut app) = make_app();

    let (gw, _gw_rx) = tokio::sync::mpsc::channel(1);

    // Gateway connect
    tx.gateway_connect("one".to_string(), GatewayTx(gw)).await;
    tx.gateway_disconnect("one".to_string()).await;
    assert_eq!(0, app.gateway_count());

    match app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        _ => panic!("expected no action from gateway connect"),
    }
    assert_eq!(1, app.gateway_count());

    match app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        _ => panic!("expected no action from gateway disconnect"),
    }
    assert_eq!(0, app.gateway_count());

    Ok(())
}

#[tokio::test]
async fn sending_non_existent_packet() -> Result {
    let (tx, mut app) = make_app();

    // Send message for packet hash never seen
    tx.uplink_send("fake-hash".into()).await;

    match app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        _ => panic!("expected no action from unknown packet hash"),
    }

    Ok(())
}

// helpers ========================================================================
fn packet_up() -> PacketUp {
    packet_up_from_gateway("")
}

fn packet_up_from_gateway(gw: &str) -> PacketUp {
    let packet = PacketRouterPacketUpV1 {
        payload: vec![],
        timestamp: 0,
        rssi: 0,
        frequency: 0,
        datarate: 0,
        snr: 0.0,
        region: 0,
        hold_time: 0,
        gateway: gw.into(),
        signature: vec![],
    };
    PacketUp::new(packet, 0)
}

fn make_app() -> (MsgSender, app::App) {
    let settings = default_settings();
    let (tx, rx) = MsgSender::new();
    let app = app::App::new(tx.clone(), rx, settings);
    (tx, app)
}

fn default_settings() -> Settings {
    Settings {
        roaming: RoamingSettings {
            helium_net_id: "C00053".to_string(),
            target_net_id: "000024".to_string(),
            sender_nsid: "sender-nsid".to_string(),
            receiver_nsid: "receiver-nsid".to_string(),
            dedup_window: DurationString::from_string("250ms".to_string()).unwrap(),
            send_pr_start_notif: false,
        },
        network: NetworkSettings {
            lns_endpoint: "localhost:8080".to_string(),
            downlink_listen: "0.0.0.0:9000".parse().unwrap(),
            uplink_listen: "0.0.0.0:9001".parse().unwrap(),
        },
        cleanup_window: DurationString::from_string("10s".to_string()).unwrap(),
    }
}
