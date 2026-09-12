-- Modelle: freie faehigkeits-/metadaten als JSONB.
-- Beispiele: {"attachment": true, "modalities": {"input": ["text","image"], "output": ["text"]},
--             "max_content_length": 131072}
ALTER TABLE models ADD COLUMN IF NOT EXISTS capabilities JSONB;
