use crate::{NtpDuration, NtpTimestamp};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum NtpLeapIndicator {
    NoWarning,
    Leap61,
    Leap59,
    Unknown,
}

impl NtpLeapIndicator {
    // This function should only ever be called with 2 bit values
    // (in the least significant position)
    fn from_bits(bits: u8) -> NtpLeapIndicator {
        match bits {
            0 => NtpLeapIndicator::NoWarning,
            1 => NtpLeapIndicator::Leap61,
            2 => NtpLeapIndicator::Leap59,
            3 => NtpLeapIndicator::Unknown,
            // This function should only ever be called from the packet parser
            // with just two bits, so this really should be unreachable
            _ => unreachable!(),
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            NtpLeapIndicator::NoWarning => 0,
            NtpLeapIndicator::Leap61 => 1,
            NtpLeapIndicator::Leap59 => 2,
            NtpLeapIndicator::Unknown => 3,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum NtpAssociationMode {
    Reserved,
    SymmetricActive,
    SymmetricPassive,
    Client,
    Server,
    Broadcast,
    Control,
    Private,
}

impl NtpAssociationMode {
    // This function should only ever be called with 3 bit values
    // (in the least significant position)
    fn from_bits(bits: u8) -> NtpAssociationMode {
        match bits {
            0 => NtpAssociationMode::Reserved,
            1 => NtpAssociationMode::SymmetricActive,
            2 => NtpAssociationMode::SymmetricPassive,
            3 => NtpAssociationMode::Client,
            4 => NtpAssociationMode::Server,
            5 => NtpAssociationMode::Broadcast,
            6 => NtpAssociationMode::Control,
            7 => NtpAssociationMode::Private,
            // This function should only ever be called from the packet parser
            // with just three bits, so this really should be unreachable
            _ => unreachable!(),
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            NtpAssociationMode::Reserved => 0,
            NtpAssociationMode::SymmetricActive => 1,
            NtpAssociationMode::SymmetricPassive => 2,
            NtpAssociationMode::Client => 3,
            NtpAssociationMode::Server => 4,
            NtpAssociationMode::Broadcast => 5,
            NtpAssociationMode::Control => 6,
            NtpAssociationMode::Private => 7,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct NtpHeader {
    pub leap: NtpLeapIndicator,
    version: u8,
    pub mode: NtpAssociationMode,
    pub stratum: u8,
    pub poll: i8,
    pub precision: i8,
    pub root_delay: NtpDuration,
    pub root_dispersion: NtpDuration,
    pub reference_id: u32,
    pub reference_timestamp: NtpTimestamp,
    pub origin_timestamp: NtpTimestamp,
    pub receive_timestamp: NtpTimestamp,
    pub transmit_timestamp: NtpTimestamp,
}

const KISS_DENY: u32 = 0x44454E59;
const KISS_RSTR: u32 = 0x52535452;
const KISS_RATE: u32 = 0x52415444;

impl NtpHeader {
    /// A new, empty NtpHeader
    pub fn new() -> Self {
        Self {
            leap: NtpLeapIndicator::NoWarning,
            version: 4,
            mode: NtpAssociationMode::Client,
            stratum: 0,
            poll: 0,
            precision: 0,
            root_delay: NtpDuration::default(),
            root_dispersion: NtpDuration::default(),
            reference_id: 0,
            reference_timestamp: NtpTimestamp::default(),
            origin_timestamp: NtpTimestamp::default(),
            receive_timestamp: NtpTimestamp::default(),
            transmit_timestamp: NtpTimestamp::default(),
        }
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn deserialize(data: &[u8; 48]) -> NtpHeader {
        NtpHeader {
            leap: NtpLeapIndicator::from_bits((data[0] & 0xC0) >> 6),
            version: (data[0] & 0x38) >> 3,
            mode: NtpAssociationMode::from_bits(data[0] & 0x07),
            stratum: data[1],
            poll: data[2] as i8,
            precision: data[3] as i8,
            root_delay: NtpDuration::from_bits_short(data[4..8].try_into().unwrap()),
            root_dispersion: NtpDuration::from_bits_short(data[8..12].try_into().unwrap()),
            reference_id: u32::from_be_bytes(data[12..16].try_into().unwrap()),
            reference_timestamp: NtpTimestamp::from_bits(data[16..24].try_into().unwrap()),
            origin_timestamp: NtpTimestamp::from_bits(data[24..32].try_into().unwrap()),
            receive_timestamp: NtpTimestamp::from_bits(data[32..40].try_into().unwrap()),
            transmit_timestamp: NtpTimestamp::from_bits(data[40..48].try_into().unwrap()),
        }
    }

    pub fn serialize(&self) -> [u8; 48] {
        // Version should only ever be set internally in this module, so
        // violations of this should never happen.
        assert!(self.version < 8);

        let root_delay = self.root_delay.to_bits_short();
        let root_dispersion = self.root_dispersion.to_bits_short();
        let reference_id = self.reference_id.to_be_bytes();
        let reference_timestamp = self.reference_timestamp.to_bits();
        let origin_timestamp = self.origin_timestamp.to_bits();
        let receive_timestamp = self.receive_timestamp.to_bits();
        let transmit_timestamp = self.transmit_timestamp.to_bits();

        [
            (self.leap.to_bits() << 6) | ((self.version & 0x07) << 3) | self.mode.to_bits(),
            self.stratum,
            self.poll as u8,
            self.precision as u8,
            root_delay[0],
            root_delay[1],
            root_delay[2],
            root_delay[3],
            root_dispersion[0],
            root_dispersion[1],
            root_dispersion[2],
            root_dispersion[3],
            reference_id[0],
            reference_id[1],
            reference_id[2],
            reference_id[3],
            reference_timestamp[0],
            reference_timestamp[1],
            reference_timestamp[2],
            reference_timestamp[3],
            reference_timestamp[4],
            reference_timestamp[5],
            reference_timestamp[6],
            reference_timestamp[7],
            origin_timestamp[0],
            origin_timestamp[1],
            origin_timestamp[2],
            origin_timestamp[3],
            origin_timestamp[4],
            origin_timestamp[5],
            origin_timestamp[6],
            origin_timestamp[7],
            receive_timestamp[0],
            receive_timestamp[1],
            receive_timestamp[2],
            receive_timestamp[3],
            receive_timestamp[4],
            receive_timestamp[5],
            receive_timestamp[6],
            receive_timestamp[7],
            transmit_timestamp[0],
            transmit_timestamp[1],
            transmit_timestamp[2],
            transmit_timestamp[3],
            transmit_timestamp[4],
            transmit_timestamp[5],
            transmit_timestamp[6],
            transmit_timestamp[7],
        ]
    }

    pub fn is_kiss(&self) -> bool {
        self.stratum == 0
    }

    pub fn is_kiss_deny(&self) -> bool {
        self.is_kiss() && self.reference_id == KISS_DENY
    }

    pub fn is_kiss_rstr(&self) -> bool {
        self.is_kiss() && self.reference_id == KISS_RSTR
    }

    pub fn is_kiss_rate(&self) -> bool {
        self.is_kiss() && self.reference_id == KISS_RATE
    }
}

impl Default for NtpHeader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_bitrep_leap() {
        for i in 0..4 as u8 {
            let a = NtpLeapIndicator::from_bits(i);
            let b = a.to_bits();
            let c = NtpLeapIndicator::from_bits(b);
            assert_eq!(i, b);
            assert_eq!(a, c);
        }
    }

    #[test]
    fn roundtrip_bitrep_mode() {
        for i in 0..8 as u8 {
            let a = NtpAssociationMode::from_bits(i);
            let b = a.to_bits();
            let c = NtpAssociationMode::from_bits(b);
            assert_eq!(i, b);
            assert_eq!(a, c);
        }
    }

    #[test]
    fn test_captured_client() {
        let packet = b"\x23\x02\x06\xe8\x00\x00\x03\xff\x00\x00\x03\x7d\x5e\xc6\x9f\x0f\xe5\xf6\x62\x98\x7b\x61\xb9\xaf\xe5\xf6\x63\x66\x7b\x64\x99\x5d\xe5\xf6\x63\x66\x81\x40\x55\x90\xe5\xf6\x63\xa8\x76\x1d\xde\x48";
        let reference = NtpHeader {
            leap: NtpLeapIndicator::NoWarning,
            version: 4,
            mode: NtpAssociationMode::Client,
            stratum: 2,
            poll: 6,
            precision: -24,
            root_delay: NtpDuration::from_fixed_int(1023 << 16),
            root_dispersion: NtpDuration::from_fixed_int(893 << 16),
            reference_id: 0x5ec69f0f,
            reference_timestamp: NtpTimestamp::from_fixed_int(0xe5f662987b61b9af),
            origin_timestamp: NtpTimestamp::from_fixed_int(0xe5f663667b64995d),
            receive_timestamp: NtpTimestamp::from_fixed_int(0xe5f6636681405590),
            transmit_timestamp: NtpTimestamp::from_fixed_int(0xe5f663a8761dde48),
        };

        assert_eq!(reference, NtpHeader::deserialize(packet));
        assert_eq!(packet[..], reference.serialize()[..]);
    }

    #[test]
    fn test_captured_server() {
        let packet = b"\x24\x02\x06\xe9\x00\x00\x02\x36\x00\x00\x03\xb7\xc0\x35\x67\x6c\xe5\xf6\x61\xfd\x6f\x16\x5f\x03\xe5\xf6\x63\xa8\x76\x19\xef\x40\xe5\xf6\x63\xa8\x79\x8c\x65\x81\xe5\xf6\x63\xa8\x79\x8e\xae\x2b";
        let reference = NtpHeader {
            leap: NtpLeapIndicator::NoWarning,
            version: 4,
            mode: NtpAssociationMode::Server,
            stratum: 2,
            poll: 6,
            precision: -23,
            root_delay: NtpDuration::from_fixed_int(566 << 16),
            root_dispersion: NtpDuration::from_fixed_int(951 << 16),
            reference_id: 0xc035676c,
            reference_timestamp: NtpTimestamp::from_fixed_int(0xe5f661fd6f165f03),
            origin_timestamp: NtpTimestamp::from_fixed_int(0xe5f663a87619ef40),
            receive_timestamp: NtpTimestamp::from_fixed_int(0xe5f663a8798c6581),
            transmit_timestamp: NtpTimestamp::from_fixed_int(0xe5f663a8798eae2b),
        };

        assert_eq!(reference, NtpHeader::deserialize(packet));
        assert_eq!(packet[..], reference.serialize()[..])
    }

    #[test]
    fn test_abstract() {
        let packet = [
            0x0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45,
            46, 47,
        ];
        let a = NtpHeader::deserialize(&packet);
        let b = a.serialize();
        let c = NtpHeader::deserialize(&b);
        assert_eq!(packet, b);
        assert_eq!(a, c);
    }

    #[test]
    fn test_packed_flags() {
        let base = [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45,
            46, 47,
        ];
        let base_structured = NtpHeader::deserialize(&base);

        for leap_type in 0..3 {
            for mode in 0..8 {
                for version in 0..8 {
                    let mut header = base_structured;
                    header.leap = NtpLeapIndicator::from_bits(leap_type);
                    header.mode = NtpAssociationMode::from_bits(mode);
                    header.version = version;

                    let data = header.serialize();
                    let copy = NtpHeader::deserialize(&data);
                    assert_eq!(header, copy);
                }
            }
        }

        for i in 0..=0xFF {
            let mut packet = base;
            packet[0] = i;
            let a = NtpHeader::deserialize(&packet);
            let b = a.serialize();
            assert_eq!(packet, b);
        }
    }
}
