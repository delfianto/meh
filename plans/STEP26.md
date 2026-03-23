# STEP 26 — Cost Tracking

## Objective
Implement per-call and cumulative cost tracking based on model pricing. Display cost in the status bar and task history.

## Prerequisites
- STEP 25 complete (token counting)
- STEP 05, 08, 09, 10 complete (providers with pricing info)

## Detailed Instructions

### 26.1 Pricing database

Add model pricing info to `ModelInfo`:
```rust
pub struct ModelInfo {
    // ... existing fields ...
    pub input_price_per_mtok: f64,   // Price per million input tokens
    pub output_price_per_mtok: f64,  // Price per million output tokens
    pub cache_read_price_per_mtok: Option<f64>,
    pub cache_write_price_per_mtok: Option<f64>,
    pub thinking_price_per_mtok: Option<f64>, // If different from output
}
```

### 26.2 Cost calculation

```rust
/// Calculate cost for a single API call.
pub fn calculate_cost(usage: &UsageInfo, pricing: &ModelInfo) -> f64 {
    let input_cost = usage.input_tokens as f64 * pricing.input_price_per_mtok / 1_000_000.0;
    let output_cost = usage.output_tokens as f64 * pricing.output_price_per_mtok / 1_000_000.0;

    let cache_read_cost = usage.cache_read_tokens
        .zip(pricing.cache_read_price_per_mtok)
        .map(|(tokens, price)| tokens as f64 * price / 1_000_000.0)
        .unwrap_or(0.0);

    let cache_write_cost = usage.cache_write_tokens
        .zip(pricing.cache_write_price_per_mtok)
        .map(|(tokens, price)| tokens as f64 * price / 1_000_000.0)
        .unwrap_or(0.0);

    let thinking_cost = usage.thinking_tokens
        .map(|tokens| {
            let price = pricing.thinking_price_per_mtok
                .unwrap_or(pricing.output_price_per_mtok);
            tokens as f64 * price / 1_000_000.0
        })
        .unwrap_or(0.0);

    input_cost + output_cost + cache_read_cost + cache_write_cost + thinking_cost
}

/// Format cost for display.
pub fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${:.4}", cost)
    } else if cost < 1.0 {
        format!("${:.3}", cost)
    } else {
        format!("${:.2}", cost)
    }
}
```

### 26.3 Known model pricing table

```rust
pub fn get_known_pricing(model_id: &str) -> Option<(f64, f64)> {
    // Returns (input_price_per_mtok, output_price_per_mtok)
    match model_id {
        // Anthropic
        "claude-opus-4-20250514" => Some((15.0, 75.0)),
        "claude-sonnet-4-20250514" => Some((3.0, 15.0)),
        "claude-haiku-4-5-20251001" => Some((0.80, 4.0)),
        // OpenAI
        "gpt-4.1" => Some((2.0, 8.0)),
        "gpt-4.1-mini" => Some((0.4, 1.6)),
        "gpt-4.1-nano" => Some((0.1, 0.4)),
        "o3" => Some((2.0, 8.0)),
        "o4-mini" => Some((1.1, 4.4)),
        // Gemini
        "gemini-2.5-pro" => Some((1.25, 10.0)),
        "gemini-2.5-flash" => Some((0.15, 0.60)),
        _ => None,
    }
}
```

### 26.4 Wire cost into status bar and task state

- Every time `StreamChunk::Usage` arrives, calculate cost
- Add to cumulative total
- Update status bar display
- Include in task history persistence

### 26.5 Cost display formatting in TUI

Status bar shows: `$0.135` for the current task's cumulative cost.

When cost exceeds thresholds, change color:
- < $0.10: green (normal)
- $0.10 - $1.00: yellow (moderate)
- > $1.00: red (expensive)

## Tests

```rust
#[cfg(test)]
mod cost_tests {
    use super::*;

    #[test]
    fn test_calculate_cost_basic() {
        let usage = UsageInfo {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            ..Default::default()
        };
        let pricing = ModelInfo {
            input_price_per_mtok: 3.0,
            output_price_per_mtok: 15.0,
            ..Default::default()
        };
        let cost = calculate_cost(&usage, &pricing);
        assert!((cost - 10.5).abs() < 0.001); // 3.0 + 7.5
    }

    #[test]
    fn test_calculate_cost_with_cache() {
        let usage = UsageInfo {
            input_tokens: 100_000,
            output_tokens: 50_000,
            cache_read_tokens: Some(80_000),
            cache_write_tokens: Some(20_000),
            ..Default::default()
        };
        let pricing = ModelInfo {
            input_price_per_mtok: 3.0,
            output_price_per_mtok: 15.0,
            cache_read_price_per_mtok: Some(0.30),
            cache_write_price_per_mtok: Some(3.75),
            ..Default::default()
        };
        let cost = calculate_cost(&usage, &pricing);
        assert!(cost > 0.0);
    }

    #[test]
    fn test_format_cost() {
        assert_eq!(format_cost(0.001), "$0.0010");
        assert_eq!(format_cost(0.05), "$0.050");
        assert_eq!(format_cost(1.5), "$1.50");
    }

    #[test]
    fn test_known_pricing() {
        assert!(get_known_pricing("claude-sonnet-4-20250514").is_some());
        assert!(get_known_pricing("unknown-model").is_none());
    }
}
```

## Acceptance Criteria
- [x] Cost calculated per API call from token counts and pricing
- [x] Cache read/write tokens priced separately
- [x] Thinking tokens priced (at output rate or custom rate)
- [x] Known model pricing table for major models
- [x] Cumulative cost tracked and displayed in status bar
- [x] Cost color-coded by threshold
- [ ] Cost saved in task history
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass
