use std::{collections::BTreeMap, fmt, io};

use blazingly_json::Value;

#[derive(Clone, Copy, Debug)]
pub(crate) struct JsonLimits {
    pub(crate) bytes: usize,
    pub(crate) nodes: usize,
    pub(crate) depth: usize,
}

#[derive(Debug)]
pub(crate) struct JsonBudget {
    limits: JsonLimits,
    nodes: usize,
    bytes: ByteCounter,
}

impl JsonBudget {
    pub(crate) const fn new(limits: JsonLimits) -> Self {
        Self {
            limits,
            nodes: 0,
            bytes: ByteCounter::new(limits.bytes),
        }
    }

    pub(crate) fn visit_map(
        &mut self,
        values: &BTreeMap<String, Value>,
    ) -> Result<(), MetadataError> {
        if values.is_empty() {
            return Ok(());
        }
        for value in values.values() {
            self.visit_value(value)?;
        }
        if let Err(error) = blazingly_json::to_writer(&mut self.bytes, values) {
            let message = if self.bytes.exceeded {
                format!(
                    "extension JSON exceeds the combined {}-byte limit",
                    self.limits.bytes
                )
            } else {
                format!("could not encode extension JSON: {error}")
            };
            return Err(MetadataError(message));
        }
        Ok(())
    }

    fn visit_value(&mut self, root: &Value) -> Result<(), MetadataError> {
        let mut stack = vec![(root, 1_usize)];
        while let Some((value, depth)) = stack.pop() {
            if self.nodes >= self.limits.nodes {
                return Err(MetadataError(format!(
                    "extension JSON has more than {} values",
                    self.limits.nodes
                )));
            }
            if depth > self.limits.depth {
                return Err(MetadataError(format!(
                    "extension JSON exceeds depth {}",
                    self.limits.depth
                )));
            }
            self.nodes += 1;
            match value {
                Value::Array(values) => self.push_children(&mut stack, values.iter(), depth)?,
                Value::Object(values) => {
                    self.push_children(&mut stack, values.values(), depth)?;
                }
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
            }
        }
        Ok(())
    }

    fn push_children<'a, I>(
        &self,
        stack: &mut Vec<(&'a Value, usize)>,
        values: I,
        depth: usize,
    ) -> Result<(), MetadataError>
    where
        I: ExactSizeIterator<Item = &'a Value>,
    {
        if values.len() == 0 {
            return Ok(());
        }
        if depth >= self.limits.depth
            || values.len()
                > self
                    .limits
                    .nodes
                    .saturating_sub(self.nodes)
                    .saturating_sub(stack.len())
        {
            return Err(MetadataError(
                "extension JSON exceeds its node or depth budget".to_owned(),
            ));
        }
        stack.extend(values.map(|value| (value, depth + 1)));
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct MetadataError(String);

impl MetadataError {
    pub(crate) fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MetadataError {}

#[derive(Debug)]
struct ByteCounter {
    total: usize,
    max: usize,
    exceeded: bool,
}

impl ByteCounter {
    const fn new(max: usize) -> Self {
        Self {
            total: 0,
            max,
            exceeded: false,
        }
    }
}

impl io::Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let Some(next) = self.total.checked_add(buffer.len()) else {
            self.exceeded = true;
            return Err(io::Error::other("extension byte count overflow"));
        };
        if next > self.max {
            self.exceeded = true;
            return Err(io::Error::other("extension byte budget exceeded"));
        }
        self.total = next;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
