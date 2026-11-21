//! Parquet output: /output/user_daily/part-N.parquet and /output/campaign_daily/part-0.parquet.
//! `day` is INT32 with the DATE logical type; strings are BYTE_ARRAY (UTF8).

use parquet::basic::Compression;
use parquet::data_type::{ByteArray, ByteArrayType, Int32Type, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

pub enum Col {
    Str(Vec<ByteArray>),
    OptStr(Vec<Option<ByteArray>>),
    Date(Vec<i32>),
    I64(Vec<i64>),
}

pub const USER_DAILY: &str = "message user_daily {
  REQUIRED BYTE_ARRAY user_id (UTF8);
  REQUIRED INT32 day (DATE);
  REQUIRED INT64 sessions;
  REQUIRED INT64 events;
  REQUIRED INT64 revenue_cents;
  OPTIONAL BYTE_ARRAY top_category (UTF8);
}";

pub const CAMPAIGN_DAILY: &str = "message campaign_daily {
  REQUIRED BYTE_ARRAY campaign_id (UTF8);
  REQUIRED BYTE_ARRAY channel (UTF8);
  REQUIRED INT32 day (DATE);
  REQUIRED INT64 sessions;
  REQUIRED INT64 purchasing_sessions;
  REQUIRED INT64 revenue_cents;
  REQUIRED INT64 assisted_sessions;
}";

/// Write one Parquet file with the given schema and columns (in schema order).
pub fn write(path: &Path, schema: &str, cols: Vec<Col>) -> parquet::errors::Result<()> {
    let schema = Arc::new(parse_message_type(schema)?);
    let props = Arc::new(
        WriterProperties::builder()
            .set_compression(Compression::UNCOMPRESSED)
            .set_dictionary_enabled(false)
            .set_statistics_enabled(parquet::file::properties::EnabledStatistics::None)
            .build(),
    );
    let mut w = SerializedFileWriter::new(File::create(path)?, schema, props)?;
    let rows = match cols.first() {
        Some(Col::Str(v)) => v.len(),
        Some(Col::OptStr(v)) => v.len(),
        Some(Col::Date(v)) => v.len(),
        Some(Col::I64(v)) => v.len(),
        None => 0,
    };
    if rows > 0 {
        let mut rg = w.next_row_group()?;
        for col in cols {
            let mut c = rg.next_column()?.expect("column count matches schema");
            match col {
                Col::Str(v) => {
                    c.typed::<ByteArrayType>().write_batch(&v, None, None)?;
                }
                Col::OptStr(v) => {
                    let def: Vec<i16> = v.iter().map(|x| x.is_some() as i16).collect();
                    let vals: Vec<ByteArray> = v.into_iter().flatten().collect();
                    c.typed::<ByteArrayType>().write_batch(&vals, Some(&def), None)?;
                }
                Col::Date(v) => {
                    c.typed::<Int32Type>().write_batch(&v, None, None)?;
                }
                Col::I64(v) => {
                    c.typed::<Int64Type>().write_batch(&v, None, None)?;
                }
            }
            c.close()?;
        }
        rg.close()?;
    }
    w.close()?;
    Ok(())
}
