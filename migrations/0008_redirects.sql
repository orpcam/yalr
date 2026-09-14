-- Temporäre, absichtsvolle Modell-Redirects (A -> B). Greifen IMMER,
-- unabhängig von der Gesundheit des Ziels — anders als die reaktiven
-- Fallbacks (fallbacks), die erst nach retryable Fehlern greifen.
-- Bewusst kein enabled-Flag: Lösen der Zeile = deaktivieren (Semantik
-- "temporär"). Ein Redirect wird maximal EINEN Hop angewandt (keine
-- Verkettung), daher die UNIQUE auf model_name.
CREATE TABLE IF NOT EXISTS redirects (
    id UUID PRIMARY KEY,
    model_name TEXT NOT NULL,
    redirect_model_name TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (model_name)
);
