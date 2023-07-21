use helium_proto::DataRate::{self, *};
pub use helium_proto::Region;

pub type DR = u8;

pub fn uplink_datarate(region: Region, datarate: DataRate) -> Option<DR> {
    match region {
        Region::As9232 => uplink_datarate(Region::As9231, datarate),
        Region::As9233 => uplink_datarate(Region::As9231, datarate),
        Region::As9234 => uplink_datarate(Region::As9231, datarate),
        Region::As9231a => uplink_datarate(Region::As9231, datarate),
        Region::As9231b => uplink_datarate(Region::As9231, datarate),
        Region::As9231c => uplink_datarate(Region::As9231, datarate),
        Region::As9231d => uplink_datarate(Region::As9231, datarate),
        Region::As9231e => uplink_datarate(Region::As9231, datarate),
        Region::As9231f => uplink_datarate(Region::As9231, datarate),
        Region::Eu868A => uplink_datarate(Region::Eu868, datarate),
        Region::Eu868B => uplink_datarate(Region::Eu868, datarate),
        Region::Eu868C => uplink_datarate(Region::Eu868, datarate),
        Region::Eu868D => uplink_datarate(Region::Eu868, datarate),
        Region::Eu868E => uplink_datarate(Region::Eu868, datarate),
        Region::Eu868F => uplink_datarate(Region::Eu868, datarate),
        Region::Au915Sb1 => uplink_datarate(Region::Au915, datarate),
        Region::Au915Sb2 => uplink_datarate(Region::Au915, datarate),
        Region::Us915 => match datarate {
            Sf12bw125 => None,
            Sf11bw125 => None,
            Sf10bw125 => Some(0),
            Sf9bw125 => Some(1),
            Sf8bw125 => Some(2),
            Sf7bw125 => Some(3),
            Sf12bw250 => None,
            Sf11bw250 => None,
            Sf10bw250 => None,
            Sf9bw250 => None,
            Sf8bw250 => None,
            Sf7bw250 => None,
            Sf12bw500 => None,
            Sf11bw500 => None,
            Sf10bw500 => None,
            Sf9bw500 => None,
            Sf8bw500 => Some(4),
            Sf7bw500 => None,
            Lrfhss1bw137 => None,
            Lrfhss2bw137 => None,
            Lrfhss1bw336 => None,
            Lrfhss2bw336 => None,
            Lrfhss1bw1523 => Some(5),
            Lrfhss2bw1523 => Some(6),
            Fsk50 => None,
        },
        Region::Eu868 => match datarate {
            Sf12bw125 => Some(0),
            Sf11bw125 => Some(1),
            Sf10bw125 => Some(2),
            Sf9bw125 => Some(3),
            Sf8bw125 => Some(4),
            Sf7bw125 => Some(5),
            Sf12bw250 => None,
            Sf11bw250 => None,
            Sf10bw250 => None,
            Sf9bw250 => None,
            Sf8bw250 => None,
            Sf7bw250 => Some(6),
            Sf12bw500 => None,
            Sf11bw500 => None,
            Sf10bw500 => None,
            Sf9bw500 => None,
            Sf8bw500 => None,
            Sf7bw500 => None,
            Lrfhss1bw137 => Some(8),
            Lrfhss2bw137 => Some(9),
            Lrfhss1bw336 => Some(10),
            Lrfhss2bw336 => Some(11),
            Lrfhss1bw1523 => None,
            Lrfhss2bw1523 => None,
            Fsk50 => Some(7),
        },
        Region::Eu433 => match datarate {
            Sf12bw125 => Some(0),
            Sf11bw125 => Some(1),
            Sf10bw125 => Some(2),
            Sf9bw125 => Some(3),
            Sf8bw125 => Some(4),
            Sf7bw125 => Some(5),
            Sf12bw250 => None,
            Sf11bw250 => None,
            Sf10bw250 => None,
            Sf9bw250 => None,
            Sf8bw250 => None,
            Sf7bw250 => Some(6),
            Sf12bw500 => None,
            Sf11bw500 => None,
            Sf10bw500 => None,
            Sf9bw500 => None,
            Sf8bw500 => None,
            Sf7bw500 => None,
            Lrfhss1bw137 => None,
            Lrfhss2bw137 => None,
            Lrfhss1bw336 => None,
            Lrfhss2bw336 => None,
            Lrfhss1bw1523 => None,
            Lrfhss2bw1523 => None,
            Fsk50 => Some(7),
        },
        Region::Cn470 => match datarate {
            Sf12bw125 => Some(0),
            Sf11bw125 => Some(1),
            Sf10bw125 => Some(2),
            Sf9bw125 => Some(3),
            Sf8bw125 => Some(4),
            Sf7bw125 => Some(5),
            Sf12bw250 => None,
            Sf11bw250 => None,
            Sf10bw250 => None,
            Sf9bw250 => None,
            Sf8bw250 => None,
            Sf7bw250 => None,
            Sf12bw500 => None,
            Sf11bw500 => None,
            Sf10bw500 => None,
            Sf9bw500 => None,
            Sf8bw500 => None,
            Sf7bw500 => Some(6),
            Lrfhss1bw137 => None,
            Lrfhss2bw137 => None,
            Lrfhss1bw336 => None,
            Lrfhss2bw336 => None,
            Lrfhss1bw1523 => None,
            Lrfhss2bw1523 => None,
            Fsk50 => Some(7),
        },
        Region::Cn779 => match datarate {
            Sf12bw125 => Some(0),
            Sf11bw125 => Some(1),
            Sf10bw125 => Some(2),
            Sf9bw125 => Some(3),
            Sf8bw125 => Some(4),
            Sf7bw125 => Some(5),
            Sf12bw250 => None,
            Sf11bw250 => None,
            Sf10bw250 => None,
            Sf9bw250 => None,
            Sf8bw250 => None,
            Sf7bw250 => Some(6),
            Sf12bw500 => None,
            Sf11bw500 => None,
            Sf10bw500 => None,
            Sf9bw500 => None,
            Sf8bw500 => None,
            Sf7bw500 => None,
            Lrfhss1bw137 => None,
            Lrfhss2bw137 => None,
            Lrfhss1bw336 => None,
            Lrfhss2bw336 => None,
            Lrfhss1bw1523 => None,
            Lrfhss2bw1523 => None,
            Fsk50 => Some(7),
        },
        Region::Au915 => match datarate {
            Sf12bw125 => Some(0),
            Sf11bw125 => Some(1),
            Sf10bw125 => Some(2),
            Sf9bw125 => Some(3),
            Sf8bw125 => Some(4),
            Sf7bw125 => Some(5),
            Sf12bw250 => None,
            Sf11bw250 => None,
            Sf10bw250 => None,
            Sf9bw250 => None,
            Sf8bw250 => None,
            Sf7bw250 => None,
            Sf12bw500 => None,
            Sf11bw500 => None,
            Sf10bw500 => None,
            Sf9bw500 => None,
            Sf8bw500 => Some(6),
            Sf7bw500 => None,
            Lrfhss1bw137 => None,
            Lrfhss2bw137 => None,
            Lrfhss1bw336 => None,
            Lrfhss2bw336 => None,
            Lrfhss1bw1523 => Some(7),
            Lrfhss2bw1523 => None,
            Fsk50 => None,
        },
        Region::As9231 => match datarate {
            Sf12bw125 => Some(0),
            Sf11bw125 => Some(1),
            Sf10bw125 => Some(2),
            Sf9bw125 => Some(3),
            Sf8bw125 => Some(4),
            Sf7bw125 => Some(5),
            Sf12bw250 => None,
            Sf11bw250 => None,
            Sf10bw250 => None,
            Sf9bw250 => None,
            Sf8bw250 => None,
            Sf7bw250 => Some(6),
            Sf12bw500 => None,
            Sf11bw500 => None,
            Sf10bw500 => None,
            Sf9bw500 => None,
            Sf8bw500 => None,
            Sf7bw500 => None,
            Lrfhss1bw137 => None,
            Lrfhss2bw137 => None,
            Lrfhss1bw336 => None,
            Lrfhss2bw336 => None,
            Lrfhss1bw1523 => None,
            Lrfhss2bw1523 => None,
            Fsk50 => Some(7),
        },
        Region::Kr920 => match datarate {
            Sf12bw125 => Some(0),
            Sf11bw125 => Some(1),
            Sf10bw125 => Some(2),
            Sf9bw125 => Some(3),
            Sf8bw125 => Some(4),
            Sf7bw125 => Some(5),
            Sf12bw250 => None,
            Sf11bw250 => None,
            Sf10bw250 => None,
            Sf9bw250 => None,
            Sf8bw250 => None,
            Sf7bw250 => None,
            Sf12bw500 => None,
            Sf11bw500 => None,
            Sf10bw500 => None,
            Sf9bw500 => None,
            Sf8bw500 => None,
            Sf7bw500 => None,
            Lrfhss1bw137 => None,
            Lrfhss2bw137 => None,
            Lrfhss1bw336 => None,
            Lrfhss2bw336 => None,
            Lrfhss1bw1523 => None,
            Lrfhss2bw1523 => None,
            Fsk50 => None,
        },
        Region::In865 => match datarate {
            Sf12bw125 => Some(0),
            Sf11bw125 => Some(1),
            Sf10bw125 => Some(2),
            Sf9bw125 => Some(3),
            Sf8bw125 => Some(4),
            Sf7bw125 => Some(5),
            Sf12bw250 => None,
            Sf11bw250 => None,
            Sf10bw250 => None,
            Sf9bw250 => None,
            Sf8bw250 => None,
            Sf7bw250 => None,
            Sf12bw500 => None,
            Sf11bw500 => None,
            Sf10bw500 => None,
            Sf9bw500 => None,
            Sf8bw500 => None,
            Sf7bw500 => None,
            Lrfhss1bw137 => None,
            Lrfhss2bw137 => None,
            Lrfhss1bw336 => None,
            Lrfhss2bw336 => None,
            Lrfhss1bw1523 => None,
            Lrfhss2bw1523 => None,
            Fsk50 => Some(7),
        },
        Region::Ru864 => match datarate {
            Sf12bw125 => Some(0),
            Sf11bw125 => Some(1),
            Sf10bw125 => Some(2),
            Sf9bw125 => Some(3),
            Sf8bw125 => Some(4),
            Sf7bw125 => Some(5),
            Sf12bw250 => None,
            Sf11bw250 => None,
            Sf10bw250 => None,
            Sf9bw250 => None,
            Sf8bw250 => None,
            Sf7bw250 => Some(6),
            Sf12bw500 => None,
            Sf11bw500 => None,
            Sf10bw500 => None,
            Sf9bw500 => None,
            Sf8bw500 => None,
            Sf7bw500 => None,
            Lrfhss1bw137 => None,
            Lrfhss2bw137 => None,
            Lrfhss1bw336 => None,
            Lrfhss2bw336 => None,
            Lrfhss1bw1523 => None,
            Lrfhss2bw1523 => None,
            Fsk50 => Some(7),
        },
        Region::Cd9001a => None,
        Region::Unknown => None,
    }
}

pub fn downlink_datarate(region: Region, dr: DR) -> Option<DataRate> {
    match region {
        Region::Unknown => None,
        Region::Cd9001a => None,
        Region::Eu868 => match dr {
            0 => Some(Sf12bw125),
            1 => Some(Sf11bw125),
            2 => Some(Sf10bw125),
            3 => Some(Sf9bw125),
            4 => Some(Sf8bw125),
            5 => Some(Sf7bw125),
            6 => Some(Sf7bw250),
            7 => Some(Fsk50),
            _ => None,
        },
        Region::Eu868A => downlink_datarate(Region::Eu868, dr),
        Region::Eu868B => downlink_datarate(Region::Eu868, dr),
        Region::Eu868C => downlink_datarate(Region::Eu868, dr),
        Region::Eu868D => downlink_datarate(Region::Eu868, dr),
        Region::Eu868E => downlink_datarate(Region::Eu868, dr),
        Region::Eu868F => downlink_datarate(Region::Eu868, dr),
        Region::Us915 => match dr {
            8 => Some(Sf12bw500),
            9 => Some(Sf11bw500),
            10 => Some(Sf10bw500),
            11 => Some(Sf9bw500),
            12 => Some(Sf8bw500),
            13 => Some(Sf7bw500),
            _ => None,
        },
        Region::Cn779 => match dr {
            0 => Some(Sf12bw125),
            1 => Some(Sf11bw125),
            2 => Some(Sf10bw125),
            3 => Some(Sf9bw125),
            4 => Some(Sf8bw125),
            5 => Some(Sf7bw125),
            6 => Some(Sf7bw500),
            7 => Some(Fsk50),
            _ => None,
        },
        Region::Eu433 => match dr {
            0 => Some(Sf12bw125),
            1 => Some(Sf11bw125),
            2 => Some(Sf10bw125),
            3 => Some(Sf9bw125),
            4 => Some(Sf8bw125),
            5 => Some(Sf7bw125),
            6 => Some(Sf7bw250),
            7 => Some(Fsk50),
            _ => None,
        },
        Region::Au915 => match dr {
            8 => Some(Sf12bw500),
            9 => Some(Sf11bw500),
            10 => Some(Sf10bw500),
            11 => Some(Sf9bw500),
            12 => Some(Sf8bw500),
            13 => Some(Sf7bw500),
            _ => None,
        },
        Region::Au915Sb1 => downlink_datarate(Region::Au915, dr),
        Region::Au915Sb2 => downlink_datarate(Region::Au915, dr),
        Region::Cn470 => match dr {
            0 => Some(Sf12bw125),
            1 => Some(Sf11bw125),
            2 => Some(Sf10bw125),
            3 => Some(Sf9bw125),
            4 => Some(Sf8bw125),
            5 => Some(Sf7bw125),
            6 => Some(Sf7bw500),
            7 => Some(Fsk50),
            _ => None,
        },
        Region::As9231 => match dr {
            0 => Some(Sf12bw125),
            1 => Some(Sf11bw125),
            2 => Some(Sf10bw125),
            3 => Some(Sf9bw125),
            4 => Some(Sf8bw125),
            5 => Some(Sf7bw125),
            6 => Some(Sf7bw250),
            7 => Some(Fsk50),
            _ => None,
        },
        Region::As9232 => downlink_datarate(Region::As9231, dr),
        Region::As9233 => downlink_datarate(Region::As9231, dr),
        Region::As9234 => downlink_datarate(Region::As9231, dr),
        Region::As9231a => downlink_datarate(Region::As9231, dr),
        Region::As9231b => downlink_datarate(Region::As9231, dr),
        Region::As9231c => downlink_datarate(Region::As9231, dr),
        Region::As9231d => downlink_datarate(Region::As9231, dr),
        Region::As9231e => downlink_datarate(Region::As9231, dr),
        Region::As9231f => downlink_datarate(Region::As9231, dr),
        Region::Kr920 => match dr {
            0 => Some(Sf12bw125),
            1 => Some(Sf11bw125),
            2 => Some(Sf10bw125),
            3 => Some(Sf9bw125),
            4 => Some(Sf8bw125),
            5 => Some(Sf7bw125),
            _ => None,
        },
        Region::In865 => match dr {
            0 => Some(Sf12bw125),
            1 => Some(Sf11bw125),
            2 => Some(Sf10bw125),
            3 => Some(Sf9bw125),
            4 => Some(Sf8bw125),
            5 => Some(Sf7bw125),
            7 => Some(Fsk50),
            _ => None,
        },
        Region::Ru864 => match dr {
            0 => Some(Sf12bw125),
            1 => Some(Sf11bw125),
            2 => Some(Sf10bw125),
            3 => Some(Sf9bw125),
            4 => Some(Sf8bw125),
            5 => Some(Sf7bw125),
            6 => Some(Sf7bw250),
            7 => Some(Fsk50),
            _ => None,
        },
    }
}
