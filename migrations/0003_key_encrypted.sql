-- Virtual keys: verschluesselter klartext (v2-AEAD, encryption_key/APP_SECRET)
-- erlaubt reveal im dashboard auch nach restarts. Legacy-keys ohne diesen
-- eintrag bleiben nicht anzeigbar (nur hash vorhanden).
ALTER TABLE virtual_keys ADD COLUMN IF NOT EXISTS key_encrypted TEXT;
