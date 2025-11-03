use anyhow::{Result, anyhow};

pub fn parse_u32_list(input: &str) -> Result<Vec<u32>> {
    let mut values = Vec::new();
    for token in input.split(|c| c == ',' || c == '\n') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = trimmed
            .parse::<u32>()
            .map_err(|_| anyhow!("无法解析为数字: {trimmed}"))?;
        values.push(value);
    }
    Ok(values)
}

pub fn parse_string_list(input: &str) -> Vec<String> {
    input
        .split(|c| c == ',' || c == '\n')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned())
        .collect()
}
