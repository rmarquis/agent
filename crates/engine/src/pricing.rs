use crate::provider::TokenUsage;

/// Per-model pricing in USD per 1M tokens.
#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl ModelPricing {
    /// Calculate the cost in USD for the given token usage.
    pub fn cost(&self, usage: &TokenUsage) -> f64 {
        let input = usage.prompt_tokens.unwrap_or(0) as f64;
        let output = usage.completion_tokens.unwrap_or(0) as f64;
        let cache_read = usage.cache_read_tokens.unwrap_or(0) as f64;
        let cache_write = usage.cache_write_tokens.unwrap_or(0) as f64;
        // Reasoning tokens are billed at the output rate.
        let reasoning = usage.reasoning_tokens.unwrap_or(0) as f64;

        (self.input * input
            + self.output * output
            + self.output * reasoning
            + self.cache_read * cache_read
            + self.cache_write * cache_write)
            / 1_000_000.0
    }
}

/// Look up built-in pricing for a model by name.
///
/// Returns `None` for unknown/local models (cost = 0).
pub fn lookup(model: &str) -> Option<ModelPricing> {
    let m = model.to_lowercase();

    // ── Anthropic ────────────────────────────────────────────────────
    if m.contains("claude") {
        if m.contains("opus") {
            return Some(ModelPricing {
                input: 15.0,
                output: 75.0,
                cache_read: 1.5,
                cache_write: 18.75,
            });
        }
        if m.contains("sonnet") {
            return Some(ModelPricing {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
            });
        }
        if m.contains("haiku") {
            return Some(ModelPricing {
                input: 0.8,
                output: 4.0,
                cache_read: 0.08,
                cache_write: 1.0,
            });
        }
    }

    // ── OpenAI ───────────────────────────────────────────────────────
    if m.contains("gpt-4.1") {
        if m.contains("nano") {
            return Some(ModelPricing {
                input: 0.10,
                output: 0.40,
                cache_read: 0.025,
                cache_write: 0.0,
            });
        }
        if m.contains("mini") {
            return Some(ModelPricing {
                input: 0.40,
                output: 1.60,
                cache_read: 0.10,
                cache_write: 0.0,
            });
        }
        return Some(ModelPricing {
            input: 2.0,
            output: 8.0,
            cache_read: 0.50,
            cache_write: 0.0,
        });
    }
    if m.contains("o3") {
        if m.contains("mini") {
            return Some(ModelPricing {
                input: 1.10,
                output: 4.40,
                cache_read: 0.275,
                cache_write: 0.0,
            });
        }
        return Some(ModelPricing {
            input: 2.0,
            output: 8.0,
            cache_read: 0.50,
            cache_write: 0.0,
        });
    }
    if m.contains("o4-mini") {
        return Some(ModelPricing {
            input: 1.10,
            output: 4.40,
            cache_read: 0.275,
            cache_write: 0.0,
        });
    }
    if m.contains("gpt-4o") {
        if m.contains("mini") {
            return Some(ModelPricing {
                input: 0.15,
                output: 0.60,
                cache_read: 0.075,
                cache_write: 0.0,
            });
        }
        return Some(ModelPricing {
            input: 2.50,
            output: 10.0,
            cache_read: 1.25,
            cache_write: 0.0,
        });
    }

    // ── Google Gemini ────────────────────────────────────────────────
    if m.contains("gemini") {
        if m.contains("2.5-pro") {
            return Some(ModelPricing {
                input: 1.25,
                output: 10.0,
                cache_read: 0.315,
                cache_write: 0.0,
            });
        }
        if m.contains("2.5-flash") {
            return Some(ModelPricing {
                input: 0.15,
                output: 0.60,
                cache_read: 0.0375,
                cache_write: 0.0,
            });
        }
    }

    // ── DeepSeek ─────────────────────────────────────────────────────
    if m.contains("deepseek") {
        if m.contains("r1") || m.contains("reasoner") {
            return Some(ModelPricing {
                input: 0.55,
                output: 2.19,
                cache_read: 0.14,
                cache_write: 0.0,
            });
        }
        return Some(ModelPricing {
            input: 0.27,
            output: 1.10,
            cache_read: 0.07,
            cache_write: 0.0,
        });
    }

    None
}

/// Build a `ModelPricing` from config overrides, falling back to the
/// built-in table, then to zero for unknown models.
pub fn resolve(model: &str, config: &crate::config::ModelConfig) -> ModelPricing {
    let builtin = lookup(model).unwrap_or(ModelPricing {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
    });
    ModelPricing {
        input: config.input_cost.unwrap_or(builtin.input),
        output: config.output_cost.unwrap_or(builtin.output),
        cache_read: config.cache_read_cost.unwrap_or(builtin.cache_read),
        cache_write: config.cache_write_cost.unwrap_or(builtin.cache_write),
    }
}

/// Format a USD cost for display.
pub fn format_cost(usd: f64) -> String {
    if usd < 0.01 {
        format!("${:.4}", usd)
    } else if usd < 1.0 {
        format!("${:.3}", usd)
    } else {
        format!("${:.2}", usd)
    }
}
