use super::{packet::PacketUp, Gateway};
use crate::Result;
use helium_proto::services::router::{
    envelope_up_v1, packet_server::Packet, packet_server::PacketServer, EnvelopeDownV1,
    EnvelopeUpV1,
};
use std::{net::SocketAddr, time::Duration};

use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

#[tonic::async_trait]
pub trait UplinkIngest: Send + Sync {
    async fn gateway_connect(&self, gateway: Gateway);
    async fn uplink_receive(&self, packet: PacketUp);
    async fn gateway_disconnect(&self, gateway: Gateway);
}

pub async fn start<S: UplinkIngest + Clone + 'static>(sender: S, addr: SocketAddr) -> Result {
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
            let mut gw = None;
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
                                if gw.is_none() {
                                    let temp_gw = Gateway::new(&packet, downlink_sender.clone());
                                    sender.gateway_connect(temp_gw.clone()).await;
                                    gw = Some(temp_gw);
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

            if let Some(gw) = gw {
                tracing::info!(uplinks, "gateway went down");
                sender.gateway_disconnect(gw).await;
            } else {
                tracing::info!("gateway with no messages sent went down");
            }
        });

        Ok(Response::new(ReceiverStream::new(downlink_receiver)))
    }
}
