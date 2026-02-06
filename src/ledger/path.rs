use serde::Deserialize;

/// Configuration from the SEP-54 `.config.json` file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreConfig {
    pub network_passphrase: String,
    pub ledgers_per_batch: u32,
    pub batches_per_partition: u32,
    pub compression: String,
    pub version: String,
}

impl StoreConfig {
    /// Compute the S3 object path for a given ledger sequence number.
    pub fn path_for_ledger(&self, ledger_sequence: u32) -> String {
        let batch_start = ledger_sequence - (ledger_sequence % self.ledgers_per_batch);
        let batch_end = batch_start + self.ledgers_per_batch - 1;

        let partition_size = self.ledgers_per_batch * self.batches_per_partition;
        let partition_start = ledger_sequence - (ledger_sequence % partition_size);
        let partition_end = partition_start + partition_size - 1;

        let partition_prefix = 0xFFFF_FFFFu32.wrapping_sub(partition_start);
        let batch_prefix = 0xFFFF_FFFFu32.wrapping_sub(batch_start);

        let partition_dir = format!(
            "{:08X}--{}-{}",
            partition_prefix, partition_start, partition_end
        );

        let batch_file = if self.ledgers_per_batch == 1 {
            format!("{:08X}--{}.xdr.zst", batch_prefix, batch_start)
        } else {
            format!(
                "{:08X}--{}-{}.xdr.zst",
                batch_prefix, batch_start, batch_end
            )
        };

        if self.batches_per_partition == 1 && self.ledgers_per_batch == 1 {
            batch_file
        } else {
            format!("{}/{}", partition_dir, batch_file)
        }
    }
}

/// Default configuration matching the pubnet S3 bucket layout.
impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            network_passphrase: "Public Global Stellar Network ; September 2015".to_string(),
            ledgers_per_batch: 1,
            batches_per_partition: 64000,
            compression: "zstd".to_string(),
            version: "0.1.0".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_single_ledger_batch() {
        let config = StoreConfig {
            ledgers_per_batch: 1,
            batches_per_partition: 64000,
            ..Default::default()
        };

        let path = config.path_for_ledger(0);
        assert_eq!(path, "FFFFFFFF--0-63999/FFFFFFFF--0.xdr.zst");

        let path = config.path_for_ledger(1);
        assert_eq!(path, "FFFFFFFF--0-63999/FFFFFFFE--1.xdr.zst");

        let path = config.path_for_ledger(64000);
        // 0xFFFFFFFF - 64000 = 0xFFFF05FF
        assert_eq!(path, "FFFF05FF--64000-127999/FFFF05FF--64000.xdr.zst");
    }

    #[test]
    fn test_path_multi_ledger_batch() {
        let config = StoreConfig {
            ledgers_per_batch: 2,
            batches_per_partition: 8,
            ..Default::default()
        };

        let path = config.path_for_ledger(0);
        assert_eq!(path, "FFFFFFFF--0-15/FFFFFFFF--0-1.xdr.zst");

        let path = config.path_for_ledger(3);
        assert_eq!(path, "FFFFFFFF--0-15/FFFFFFFD--2-3.xdr.zst");

        let path = config.path_for_ledger(16);
        // 0xFFFFFFFF - 16 = 0xFFFFFFEF
        assert_eq!(path, "FFFFFFEF--16-31/FFFFFFEF--16-17.xdr.zst");
    }
}
