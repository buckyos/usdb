use rust_rocksdb as rocksdb;
use rocksdb::properties::ESTIMATE_NUM_KEYS;

pub fn get_approx_cf_key_count(db: &rocksdb::DB, cf: &str) -> Result<u64, String> {
    let cf = db.cf_handle(cf).ok_or_else(|| {
        let msg = format!("Column family {} not found", cf);
        error!("{}", msg);
        msg
    })?;

    match db.property_int_value_cf(cf, ESTIMATE_NUM_KEYS) {
        Ok(Some(value)) => Ok(value),
        Ok(None) => {
            let msg = format!("Property '{}' not found", ESTIMATE_NUM_KEYS);
            error!("{}", msg);
            Err(msg)
        }
        Err(e) => {
            let msg = format!("Failed to get property '{}': {}", ESTIMATE_NUM_KEYS, e);
            error!("{}", msg);
            Err(msg)
        }
    }
}
