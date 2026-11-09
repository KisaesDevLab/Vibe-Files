//! The parts of `twain.h` (TWAIN 2.5, TWAIN Working Group) this app uses. On Windows the
//! header packs every structure to 2 bytes (`#pragma pack (2)`); the sizes are checked below
//! against the header's layout.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::c_void;

pub type Handle = *mut c_void;

/// `DSM_Entry` / `DS_Entry` / the application callback (`TW_CALLINGSTYLE` is `PASCAL`, which
/// is the platform convention on x64).
pub type EntryProc = unsafe extern "system" fn(
    origin: *mut Identity,
    dest: *mut Identity,
    dg: u32,
    dat: u16,
    msg: u16,
    data: *mut c_void,
) -> u16;
pub type MemAllocate = unsafe extern "system" fn(size: u32) -> Handle;
pub type MemFree = unsafe extern "system" fn(h: Handle);
pub type MemLock = unsafe extern "system" fn(h: Handle) -> *mut c_void;
pub type MemUnlock = unsafe extern "system" fn(h: Handle);

pub type Str32 = [u8; 34];

#[repr(C, packed(2))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fix32 {
    pub whole: i16,
    pub frac: u16,
}

impl Fix32 {
    pub fn from_f32(v: f32) -> Self {
        let n = (v * 65536.0 + 0.5) as i32;
        Fix32 {
            whole: (n >> 16) as i16,
            frac: (n & 0xffff) as u16,
        }
    }
    pub fn to_f32(self) -> f32 {
        self.whole as f32 + self.frac as f32 / 65536.0
    }
    /// As the 32-bit `Item` of a `TW_ONEVALUE` (the struct's bytes, little-endian).
    pub fn to_item(self) -> u32 {
        (self.whole as u16 as u32) | ((self.frac as u32) << 16)
    }
    pub fn from_item(v: u32) -> Self {
        Fix32 {
            whole: (v & 0xffff) as u16 as i16,
            frac: (v >> 16) as u16,
        }
    }
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub language: u16,
    pub country: u16,
    pub info: Str32,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct Identity {
    pub id: u32,
    pub version: Version,
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub supported_groups: u32,
    pub manufacturer: Str32,
    pub product_family: Str32,
    pub product_name: Str32,
}

impl Default for Identity {
    fn default() -> Self {
        Identity {
            id: 0,
            version: Version {
                major: 0,
                minor: 0,
                language: 0,
                country: 0,
                info: [0; 34],
            },
            protocol_major: 0,
            protocol_minor: 0,
            supported_groups: 0,
            manufacturer: [0; 34],
            product_family: [0; 34],
            product_name: [0; 34],
        }
    }
}

/// A NUL-terminated `TW_STR32` from `s` (cut to 33 bytes).
pub fn str32(s: &str) -> Str32 {
    let mut out = [0u8; 34];
    for (o, b) in out.iter_mut().zip(s.bytes().take(33)) {
        *o = b;
    }
    out
}

/// A `TW_STR32` as text (ANSI; non-ASCII bytes read as Latin-1).
pub fn read_str32(s: &Str32) -> String {
    s.iter()
        .take_while(|b| **b != 0)
        .map(|&b| b as char)
        .collect::<String>()
        .trim()
        .to_string()
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct Capability {
    pub cap: u16,
    pub con_type: u16,
    pub container: Handle,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct OneValue {
    pub item_type: u16,
    pub item: u32,
}

/// Header of a `TW_ENUMERATION`; `ItemList` follows.
#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct EnumerationHead {
    pub item_type: u16,
    pub num_items: u32,
    pub current_index: u32,
    pub default_index: u32,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct Range {
    pub item_type: u16,
    pub min: u32,
    pub max: u32,
    pub step: u32,
    pub default: u32,
    pub current: u32,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct UserInterface {
    pub show_ui: u16,
    pub modal_ui: u16,
    pub parent: Handle,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy, Default)]
pub struct PendingXfers {
    pub count: u16,
    pub eoj: u32,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy, Default)]
pub struct SetupMemXfer {
    pub min_buf_size: u32,
    pub max_buf_size: u32,
    pub preferred: u32,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct Memory {
    pub flags: u32,
    pub length: u32,
    pub the_mem: *mut c_void,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct ImageMemXfer {
    pub compression: u16,
    pub bytes_per_row: u32,
    pub columns: u32,
    pub rows: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub bytes_written: u32,
    pub memory: Memory,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy, Default)]
pub struct ImageInfo {
    pub x_resolution: Fix32,
    pub y_resolution: Fix32,
    pub image_width: i32,
    pub image_length: i32,
    pub samples_per_pixel: i16,
    pub bits_per_sample: [i16; 8],
    pub bits_per_pixel: i16,
    pub planar: u16,
    pub pixel_type: i16,
    pub compression: u16,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct Event {
    pub event: *mut c_void,
    pub message: u16,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct EntryPoint {
    pub size: u32,
    pub dsm_entry: Option<EntryProc>,
    pub mem_allocate: Option<MemAllocate>,
    pub mem_free: Option<MemFree>,
    pub mem_lock: Option<MemLock>,
    pub mem_unlock: Option<MemUnlock>,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub struct Callback2 {
    pub proc_: *mut c_void,
    pub ref_con: usize,
    pub message: i16,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy, Default)]
pub struct Status {
    pub condition_code: u16,
    pub data: u16,
}

// Sizes under `#pragma pack (2)` on Windows: the same for both, except the structures holding
// a pointer or handle (8 bytes on 64-bit, 4 on 32-bit: the 32-bit scanning helper).
const _: () = {
    assert!(size_of::<Fix32>() == 4);
    assert!(size_of::<Version>() == 42);
    assert!(size_of::<Identity>() == 156);
    assert!(size_of::<OneValue>() == 6);
    assert!(size_of::<EnumerationHead>() == 14);
    assert!(size_of::<Range>() == 22);
    assert!(size_of::<PendingXfers>() == 6);
    assert!(size_of::<SetupMemXfer>() == 12);
    assert!(size_of::<ImageInfo>() == 42);
    assert!(size_of::<Status>() == 4);
};
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<Capability>() == 12);
    assert!(size_of::<UserInterface>() == 12);
    assert!(size_of::<Memory>() == 16);
    assert!(size_of::<ImageMemXfer>() == 42);
    assert!(size_of::<Event>() == 10);
    assert!(size_of::<EntryPoint>() == 44);
    assert!(size_of::<Callback2>() == 18);
};
#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(size_of::<Capability>() == 8);
    assert!(size_of::<UserInterface>() == 8);
    assert!(size_of::<Memory>() == 12);
    assert!(size_of::<ImageMemXfer>() == 38);
    assert!(size_of::<Event>() == 6);
    assert!(size_of::<EntryPoint>() == 24);
    assert!(size_of::<Callback2>() == 10);
};

// Data groups and flags.
pub const DG_CONTROL: u32 = 0x0001;
pub const DG_IMAGE: u32 = 0x0002;
pub const DF_DSM2: u32 = 0x1000_0000;
pub const DF_APP2: u32 = 0x2000_0000;
pub const DF_DS2: u32 = 0x4000_0000;

// Data argument types.
pub const DAT_NULL: u16 = 0x0000;
pub const DAT_CAPABILITY: u16 = 0x0001;
pub const DAT_EVENT: u16 = 0x0002;
pub const DAT_IDENTITY: u16 = 0x0003;
pub const DAT_PARENT: u16 = 0x0004;
pub const DAT_PENDINGXFERS: u16 = 0x0005;
pub const DAT_SETUPMEMXFER: u16 = 0x0006;
pub const DAT_STATUS: u16 = 0x0008;
pub const DAT_USERINTERFACE: u16 = 0x0009;
pub const DAT_CALLBACK2: u16 = 0x0012;
pub const DAT_IMAGEINFO: u16 = 0x0101;
pub const DAT_IMAGEMEMXFER: u16 = 0x0103;
pub const DAT_ENTRYPOINT: u16 = 0x0403;

// Messages.
pub const MSG_NULL: u16 = 0x0000;
pub const MSG_GET: u16 = 0x0001;
pub const MSG_GETCURRENT: u16 = 0x0002;
pub const MSG_GETDEFAULT: u16 = 0x0003;
pub const MSG_GETFIRST: u16 = 0x0004;
pub const MSG_GETNEXT: u16 = 0x0005;
pub const MSG_SET: u16 = 0x0006;
pub const MSG_RESET: u16 = 0x0007;
pub const MSG_XFERREADY: u16 = 0x0101;
pub const MSG_CLOSEDSREQ: u16 = 0x0102;
pub const MSG_CLOSEDSOK: u16 = 0x0103;
pub const MSG_DEVICEEVENT: u16 = 0x0104;
pub const MSG_OPENDSM: u16 = 0x0301;
pub const MSG_CLOSEDSM: u16 = 0x0302;
pub const MSG_OPENDS: u16 = 0x0401;
pub const MSG_CLOSEDS: u16 = 0x0402;
pub const MSG_DISABLEDS: u16 = 0x0501;
pub const MSG_ENABLEDS: u16 = 0x0502;
pub const MSG_PROCESSEVENT: u16 = 0x0601;
pub const MSG_ENDXFER: u16 = 0x0701;
pub const MSG_STOPFEEDER: u16 = 0x0702;
pub const MSG_REGISTER_CALLBACK: u16 = 0x0902;

// Return codes.
pub const TWRC_SUCCESS: u16 = 0;
pub const TWRC_FAILURE: u16 = 1;
pub const TWRC_CHECKSTATUS: u16 = 2;
pub const TWRC_CANCEL: u16 = 3;
pub const TWRC_DSEVENT: u16 = 4;
pub const TWRC_NOTDSEVENT: u16 = 5;
pub const TWRC_XFERDONE: u16 = 6;
pub const TWRC_ENDOFLIST: u16 = 7;

// Condition codes.
pub const TWCC_SUCCESS: u16 = 0;
pub const TWCC_BUMMER: u16 = 1;
pub const TWCC_LOWMEMORY: u16 = 2;
pub const TWCC_NODS: u16 = 3;
pub const TWCC_MAXCONNECTIONS: u16 = 4;
pub const TWCC_OPERATIONERROR: u16 = 5;
pub const TWCC_BADCAP: u16 = 6;
pub const TWCC_BADPROTOCOL: u16 = 9;
pub const TWCC_BADVALUE: u16 = 10;
pub const TWCC_SEQERROR: u16 = 11;
pub const TWCC_BADDEST: u16 = 12;
pub const TWCC_CAPUNSUPPORTED: u16 = 13;
pub const TWCC_CAPBADOPERATION: u16 = 14;
pub const TWCC_CAPSEQERROR: u16 = 15;
pub const TWCC_DENIED: u16 = 16;
pub const TWCC_PAPERJAM: u16 = 20;
pub const TWCC_PAPERDOUBLEFEED: u16 = 21;
pub const TWCC_CHECKDEVICEONLINE: u16 = 23;
pub const TWCC_INTERLOCK: u16 = 24;
pub const TWCC_DAMAGEDCORNER: u16 = 25;
pub const TWCC_NOMEDIA: u16 = 29;

// Containers and item types.
pub const TWON_ARRAY: u16 = 3;
pub const TWON_ENUMERATION: u16 = 4;
pub const TWON_ONEVALUE: u16 = 5;
pub const TWON_RANGE: u16 = 6;
pub const TWON_DONTCARE16: u16 = 0xffff;
pub const TWTY_INT8: u16 = 0x0000;
pub const TWTY_INT16: u16 = 0x0001;
pub const TWTY_INT32: u16 = 0x0002;
pub const TWTY_UINT8: u16 = 0x0003;
pub const TWTY_UINT16: u16 = 0x0004;
pub const TWTY_UINT32: u16 = 0x0005;
pub const TWTY_BOOL: u16 = 0x0006;
pub const TWTY_FIX32: u16 = 0x0007;

/// Bytes per item of a container's item type (`None` for types this app never reads).
pub fn item_size(t: u16) -> Option<usize> {
    Some(match t {
        TWTY_INT8 | TWTY_UINT8 => 1,
        TWTY_INT16 | TWTY_UINT16 | TWTY_BOOL => 2,
        TWTY_INT32 | TWTY_UINT32 | TWTY_FIX32 => 4,
        _ => return None,
    })
}

// Capabilities.
pub const CAP_XFERCOUNT: u16 = 0x0001;
pub const CAP_FEEDERENABLED: u16 = 0x1002;
pub const CAP_FEEDERLOADED: u16 = 0x1003;
pub const CAP_AUTOFEED: u16 = 0x1007;
pub const CAP_INDICATORS: u16 = 0x100b;
pub const CAP_DUPLEX: u16 = 0x1012;
pub const CAP_DUPLEXENABLED: u16 = 0x1013;
pub const ICAP_COMPRESSION: u16 = 0x0100;
pub const ICAP_PIXELTYPE: u16 = 0x0101;
pub const ICAP_UNITS: u16 = 0x0102;
pub const ICAP_XFERMECH: u16 = 0x0103;
pub const ICAP_XRESOLUTION: u16 = 0x1118;
pub const ICAP_YRESOLUTION: u16 = 0x1119;
pub const ICAP_PIXELFLAVOR: u16 = 0x111f;
pub const ICAP_BITDEPTH: u16 = 0x112b;
pub const ICAP_AUTODISCARDBLANKPAGES: u16 = 0x1134;

pub const TWSX_MEMORY: u16 = 2;
pub const TWCP_NONE: u16 = 0;
pub const TWUN_INCHES: u16 = 0;
pub const TWPT_BW: u16 = 0;
pub const TWPT_GRAY: u16 = 1;
pub const TWPT_RGB: u16 = 2;
pub const TWPF_CHOCOLATE: u16 = 0;
pub const TWBP_DISABLE: i32 = -2;
pub const TWBP_AUTO: i32 = -1;

pub const TWMF_APPOWNS: u32 = 0x0001;
pub const TWMF_POINTER: u32 = 0x0008;

pub const TWLG_USA: u16 = 13;
pub const TWCY_USA: u16 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix32_round_trips() {
        for v in [0.0, 1.0, 300.0, 200.5, -1.25] {
            let f = Fix32::from_f32(v);
            assert!((f.to_f32() - v).abs() < 1e-4, "{v}");
            assert_eq!(Fix32::from_item(f.to_item()), f);
        }
        assert_eq!(Fix32::from_f32(300.0).to_item(), 300);
    }

    #[test]
    fn str32_is_terminated_and_cut() {
        let s = str32(&"x".repeat(50));
        assert_eq!(s[33], 0);
        assert_eq!(read_str32(&s).len(), 33);
        assert_eq!(read_str32(&str32("fi-7160")), "fi-7160");
    }
}
