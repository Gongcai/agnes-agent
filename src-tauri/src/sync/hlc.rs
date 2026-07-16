use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HybridTimestamp {
    pub physical_ms: u64,
    pub counter: u32,
    pub node: String,
}

impl HybridTimestamp {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut parts = value.splitn(3, '-');
        let physical_ms = parts
            .next()
            .ok_or_else(|| "HLC is missing physical time".to_string())?
            .parse::<u64>()
            .map_err(|_| "HLC physical time is invalid".to_string())?;
        let counter = parts
            .next()
            .ok_or_else(|| "HLC is missing counter".to_string())?
            .parse::<u32>()
            .map_err(|_| "HLC counter is invalid".to_string())?;
        let node = parts
            .next()
            .filter(|node| !node.is_empty())
            .ok_or_else(|| "HLC is missing node".to_string())?
            .to_string();
        Ok(Self {
            physical_ms,
            counter,
            node,
        })
    }

    pub fn tick(last: Option<&str>, wall_ms: u64, node: &str) -> Result<Self, String> {
        Self::merge(last, None, wall_ms, node)
    }

    pub fn merge(
        last: Option<&str>,
        remote: Option<&str>,
        wall_ms: u64,
        node: &str,
    ) -> Result<Self, String> {
        if node.is_empty() || node.contains('-') {
            return Err("HLC node must be non-empty and cannot contain '-'".to_string());
        }
        let local = last.map(Self::parse).transpose()?;
        let remote = remote.map(Self::parse).transpose()?;
        let local_ms = local.as_ref().map_or(0, |value| value.physical_ms);
        let remote_ms = remote.as_ref().map_or(0, |value| value.physical_ms);
        let physical_ms = wall_ms.max(local_ms).max(remote_ms);
        let counter = match (
            local
                .as_ref()
                .filter(|value| value.physical_ms == physical_ms),
            remote
                .as_ref()
                .filter(|value| value.physical_ms == physical_ms),
        ) {
            (Some(local), Some(remote)) => local.counter.max(remote.counter).saturating_add(1),
            (Some(local), None) => local.counter.saturating_add(1),
            (None, Some(remote)) => remote.counter.saturating_add(1),
            (None, None) => 0,
        };
        Ok(Self {
            physical_ms,
            counter,
            node: node.to_string(),
        })
    }
}

impl fmt::Display for HybridTimestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}-{:04}-{}",
            self.physical_ms, self.counter, self.node
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_forward_and_survives_clock_rollback() {
        let first = HybridTimestamp::tick(None, 1_000, "device01").unwrap();
        assert_eq!(first.to_string(), "1000-0000-device01");
        let same_wall = HybridTimestamp::tick(Some(&first.to_string()), 1_000, "device01").unwrap();
        assert_eq!(same_wall.to_string(), "1000-0001-device01");
        let rollback =
            HybridTimestamp::tick(Some(&same_wall.to_string()), 900, "device01").unwrap();
        assert_eq!(rollback.to_string(), "1000-0002-device01");
        let forward =
            HybridTimestamp::tick(Some(&rollback.to_string()), 2_000, "device01").unwrap();
        assert_eq!(forward.to_string(), "2000-0000-device01");
    }

    #[test]
    fn merges_remote_causality_deterministically() {
        let merged = HybridTimestamp::merge(
            Some("1000-0002-device01"),
            Some("1000-0007-device02"),
            950,
            "device01",
        )
        .unwrap();
        assert_eq!(merged.to_string(), "1000-0008-device01");
    }

    #[test]
    fn rejects_malformed_values_and_nodes() {
        assert!(HybridTimestamp::parse("not-an-hlc").is_err());
        assert!(HybridTimestamp::tick(None, 1, "bad-node").is_err());
    }
}
