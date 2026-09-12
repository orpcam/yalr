-- Reveal-Schutz: erster teil des key-hashes als hint.
-- Client muss diesen beim POST /keys/:id/reveal mitschicken.
ALTER TABLE virtual_keys ADD COLUMN IF NOT EXISTS key_hint TEXT NOT NULL DEFAULT '';
