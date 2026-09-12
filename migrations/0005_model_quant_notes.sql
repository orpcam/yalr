-- Optionale modell-metafelder: quantisierung (z.b. "FP8", "AWQ-INT4")
-- und freie notizen. Beide rein informativ fuer das dashboard.
ALTER TABLE models ADD COLUMN IF NOT EXISTS quantization TEXT;
ALTER TABLE models ADD COLUMN IF NOT EXISTS notes TEXT;
