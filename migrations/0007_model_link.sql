-- Optionaler link pro modell (z.b. model-card, dokumentation, blog-post).
ALTER TABLE models ADD COLUMN IF NOT EXISTS link TEXT;
