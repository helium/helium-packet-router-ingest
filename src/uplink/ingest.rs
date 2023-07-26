use std::{net::SocketAddr, time::Duration};

use crate::{uplink::packet::PacketUpTrait, Result};
use helium_proto::services::router::{
    envelope_down_v1, envelope_up_v1, packet_server::Packet, packet_server::PacketServer,
    EnvelopeDownV1, EnvelopeUpV1, PacketRouterPacketDownV1,
};
use tokio::sync::mpsc::Sender;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use super::packet::PacketUp;

#[derive(Debug, Clone)]
pub struct GatewayID {
    pub b58: String,
    pub mac: String,
    pub tx: GatewayTx,
}

#[tonic::async_trait]
pub trait UplinkIngest: Send + Sync {
    async fn gateway_connect(&self, gateway: GatewayID);
    async fn uplink_receive(&self, packet: PacketUp);
    async fn gateway_disconnect(&self, gateway_b58: String);
}

pub async fn start<S: UplinkIngest + Clone>(sender: S, addr: SocketAddr) -> Result {
    tonic::transport::Server::builder()
        .add_service(PacketServer::new(Gateways::new(sender)))
        .serve(addr)
        .await
        .map_err(anyhow::Error::from)
}

struct Gateways<S>
where
    S: UplinkIngest + 'static,
{
    sender: S,
}

impl<S> Gateways<S>
where
    S: UplinkIngest + 'static,
{
    pub fn new(sender: S) -> Self {
        Self { sender }
    }
}

#[derive(Debug, Clone)]
pub struct GatewayTx(pub Sender<Result<EnvelopeDownV1, Status>>);

impl GatewayTx {
    pub async fn send_downlink(&self, downlink: PacketRouterPacketDownV1) {
        let _ = self
            .0
            .send(Ok(EnvelopeDownV1 {
                data: Some(envelope_down_v1::Data::Packet(downlink)),
            }))
            .await;
    }
}

#[tonic::async_trait]
impl<S> Packet for Gateways<S>
where
    S: UplinkIngest + 'static + Clone,
{
    type routeStream = ReceiverStream<Result<EnvelopeDownV1, Status>>;

    async fn route(
        &self,
        request: Request<Streaming<EnvelopeUpV1>>,
    ) -> Result<Response<Self::routeStream>, Status> {
        let sender = self.sender.clone();
        let mut req = request.into_inner();

        let (downlink_sender, downlink_receiver) = tokio::sync::mpsc::channel(128);
        tracing::info!("connection");

        tokio::spawn(async move {
            let mut gateway_b58 = None;
            let mut uplinks = 0;

            loop {
                match tokio::time::timeout(Duration::from_secs(10), req.message()).await {
                    Err(_) => {
                        // Timeout occurred, check if the downlink channel has been dropped
                        if downlink_sender.is_closed() {
                            tracing::info!("downlink channel was dropped");
                            break;
                        }
                    }
                    Ok(uplink_msg) => match uplink_msg {
                        Err(err) => {
                            tracing::trace!("uplink_msg error: {err}");
                            break;
                        }
                        Ok(None) => {
                            tracing::info!("client shutdown connection");
                            break;
                        }
                        Ok(Some(env_up)) => {
                            if let Some(envelope_up_v1::Data::Packet(packet)) = env_up.data {
                                let packet: PacketUp = packet.into();
                                if gateway_b58.is_none() {
                                    let b58 = packet.gateway_b58();
                                    gateway_b58 = Some(b58.clone());
                                    let gw = GatewayID {
                                        b58: packet.gateway_b58(),
                                        mac: packet.gateway_mac_str(),
                                        tx: GatewayTx(downlink_sender.clone()),
                                    };
                                    sender.gateway_connect(gw).await;
                                }
                                uplinks += 1;
                                sender.uplink_receive(packet).await;
                            } else {
                                tracing::warn!(?env_up.data, "ignoring message");
                            }
                        }
                    },
                }
            }

            if let Some(gw_b58) = gateway_b58 {
                tracing::info!(uplinks, "gateway went down");
                sender.gateway_disconnect(gw_b58).await;
            } else {
                tracing::info!("gateway with no messages sent went down");
            }
        });

        Ok(Response::new(ReceiverStream::new(downlink_receiver)))
    }
}
