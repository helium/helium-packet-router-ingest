use crate::uplink::{PacketHash, PacketUp, PacketUpTrait};
use std::collections::HashMap;

pub struct Deduplicator {
    packets: HashMap<PacketHash, Vec<PacketUp>>,
}

pub enum HandlePacket {
    New(PacketHash),
    Existing,
}

impl Deduplicator {
    pub fn new() -> Self {
        Self {
            packets: HashMap::new(),
        }
    }

    /// If we've never seen a packet before we will:
    /// - Insert the packet to collect the rest.
    /// - Wait for the DedupWindow, then ask for the packet to be sent.
    /// - Wait for the cleanupWindow, then remove all copies of the packet.
    pub fn handle_packet(&mut self, packet: PacketUp) -> HandlePacket {
        let mut result = HandlePacket::Existing;
        let hash = packet.hash();
        self.packets
            .entry(hash.clone())
            .and_modify(|bucket| bucket.push(packet.clone()))
            .or_insert_with(|| {
                result = HandlePacket::New(hash);
                vec![packet]
            });
        result
    }

    pub fn get_packets(&self, hash: &PacketHash) -> Vec<PacketUp> {
        self.packets
            .get(hash)
            .expect("packets exist for hash")
            .to_owned()
    }

    pub fn remove_packets(&mut self, hash: &PacketHash) {
        self.packets.remove(hash);
    }
}
