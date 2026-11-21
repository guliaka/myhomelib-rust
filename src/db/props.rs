//! Collection property ids, collection type codes and the schema-version stamp — ported verbatim from
//! the original's `unit_Consts.pas` so a collection file written here is accepted by the original (and
//! the other way round).

use chrono::Local;

/// The schema generation the original checks in `IsValidCollection`.
pub const DATABASE_VERSION: &str = "{FEC8CB6F-300A-4b92-86D1-7B40867F782B}";

const PROP_CLASS_SYSTEM: i64 = 0x1000_0000;
const PROP_CLASS_COLLECTION: i64 = 0x2000_0000;
const PROP_CLASS_BOTH: i64 = PROP_CLASS_SYSTEM | PROP_CLASS_COLLECTION;

const PROP_TYPE_INTEGER: i64 = 0x0001_0000;
const PROP_TYPE_DATETIME: i64 = 0x0002_0000;
const PROP_TYPE_STRING: i64 = 0x0004_0000;

pub const PROP_CODE: i64 = PROP_CLASS_BOTH | PROP_TYPE_INTEGER | 0x0002;
pub const PROP_URL: i64 = PROP_CLASS_BOTH | PROP_TYPE_STRING | 0x0007;
pub const PROP_CONNECTIONSCRIPT: i64 = PROP_CLASS_BOTH | PROP_TYPE_STRING | 0x0008;
pub const PROP_DATAVERSION: i64 = PROP_CLASS_BOTH | PROP_TYPE_INTEGER | 0x0009;
pub const PROP_NOTES: i64 = PROP_CLASS_COLLECTION | PROP_TYPE_STRING | 0x000A;
pub const PROP_CREATIONDATE: i64 = PROP_CLASS_COLLECTION | PROP_TYPE_DATETIME | 0x000B;
pub const PROP_SCHEMA_VERSION: i64 = PROP_CLASS_COLLECTION | PROP_TYPE_STRING | 0x000C;

// Collection type codes (`COLLECTION_TYPE`).
pub const CONTENT_NONFB: i64 = 0x0000_0001;
pub const LIBRARY_EXTERNAL: i64 = 0x0001_0000;
pub const LOCATION_ONLINE: i64 = 0x0800_0000;

pub const CT_PRIVATE_FB: i64 = 0;
pub const CT_PRIVATE_NONFB: i64 = CONTENT_NONFB;
pub const CT_EXTERNAL_LOCAL_FB: i64 = LIBRARY_EXTERNAL;
pub const CT_EXTERNAL_ONLINE_FB: i64 = LOCATION_ONLINE | LIBRARY_EXTERNAL;
pub const CT_EXTERNAL_LOCAL_NONFB: i64 = LIBRARY_EXTERNAL | CONTENT_NONFB;
pub const CT_EXTERNAL_ONLINE_NONFB: i64 = LOCATION_ONLINE | LIBRARY_EXTERNAL | CONTENT_NONFB;

/// True for collections whose books are downloaded on demand (the original's `isOnlineCollection`).
pub fn is_online(code: i64) -> bool {
    code & LOCATION_ONLINE == LOCATION_ONLINE
}

/// True for collections holding non-FB2 files (the original's `isNonFB2Collection`).
pub fn is_non_fb2(code: i64) -> bool {
    code & CONTENT_NONFB == CONTENT_NONFB
}

/// Human-readable collection type, for the collection list / properties dialog.
pub fn code_label(code: i64) -> &'static str {
    match code {
        CT_PRIVATE_FB => "Личная коллекция (FB2)",
        CT_PRIVATE_NONFB => "Личная коллекция (не FB2)",
        CT_EXTERNAL_LOCAL_FB => "Внешняя локальная библиотека (FB2)",
        CT_EXTERNAL_ONLINE_FB => "Внешняя онлайн-библиотека (FB2)",
        CT_EXTERNAL_LOCAL_NONFB => "Внешняя локальная библиотека (не FB2)",
        CT_EXTERNAL_ONLINE_NONFB => "Внешняя онлайн-библиотека (не FB2)",
        _ => "Коллекция",
    }
}

/// The original's SQLite date-time text format (`yyyy-mm-dd hh:nn:ss.zzz`, 23 chars) for "now".
pub fn now_string() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// A calendar date as the original stores it. Its `DateTimeToStr` drops the time part when it is
/// midnight, so INPX `DATE` fields end up as plain `yyyy-mm-dd` (verified against a collection
/// written by the original).
pub fn date_string(year: i32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}
