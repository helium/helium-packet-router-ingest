use hpr_http_rs::{
    gwmp::{self, app::UpdateAction, settings::GwmpSettings},
    region::Region,
    uplink::{ingest::UplinkIngest, Gateway, GatewayB58, GatewayMac, GatewayTx},
    Result,
};
use tokio::sync::mpsc::{error::TryRecvError, Receiver};

#[tokio::test]
async fn gwmp_gateway_connect_disconnect() -> Result {
    let (tx, mut app) = make_gwmp_app();

    let (gw, mut gw_rx) = tokio::sync::mpsc::channel(1);

    let gateway = Gateway {
        b58: GatewayB58("13jnwnZYLDgw9Kd4zp33cyx4tBNQJ4jMoNvHTiFyvUkAgkhQUz9".to_string()),
        mac: GatewayMac("86213a0f50bce10e".to_string()),
        region: Region::Us915,
        tx: GatewayTx(gw),
    };

    // Gateway connect
    tx.gateway_connect(gateway.clone()).await;
    tx.gateway_disconnect(gateway).await;
    assert_eq!(0, app.gateway_count());

    match gwmp::app::handle_single_message(&mut app).await {
        UpdateAction::NewForwarder { .. } => (),
        action => anyhow::bail!("expected new client from gateway connect got: {action:?}"),
    }
    assert_eq!(1, app.gateway_count());
    assert!(channel_is_open(&mut gw_rx)?);

    match gwmp::app::handle_single_message(&mut app).await {
        UpdateAction::Noop => (),
        action => anyhow::bail!("expected no action from gateway disconnect got: {action:?}"),
    }
    assert_eq!(0, app.gateway_count());
    assert!(channel_is_closed(&mut gw_rx)?);

    Ok(())
}

// helpers ========================================================================
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
