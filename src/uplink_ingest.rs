use std::{net::SocketAddr, time::Duration};

use crate::{actions::MsgSender, packet::PacketUp, Result};
use helium_proto::services::router::{
    envelope_down_v1, envelope_up_v1, packet_server::Packet, packet_server::PacketServer,
    EnvelopeDownV1, EnvelopeUpV1, PacketRouterPacketDownV1,
};
use tokio::sync::mpsc::Sender;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::instrument;

#[instrument]
pub fn start(sender: MsgSender, addr: SocketAddr) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(PacketServer::new(Gateways::new(sender)))
            .serve(addr)
            .await
            .unwrap();
    })
}

#[derive(Debug, Clone)]
pub struct GatewayTx(pub Sender<Result<EnvelopeDownV1, Status>>);
pub type GatewayID = String;

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

#[derive(Debug)]
struct Gateways {
    sender: MsgSender,
}

impl Gateways {
    pub fn new(sender: MsgSender) -> Self {
        Self { sender }
    }
}

#[tonic::async_trait]
impl Packet for Gateways {
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
                                    sender
                                        .gateway_connect(
                                            &packet,
                                            GatewayTx(downlink_sender.clone()),
                                        )
                                        .await;
                                    gateway_b58 = Some(packet.gateway_b58())
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
