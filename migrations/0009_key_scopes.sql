-- Key-Scopes: optionale Allow-Listen pro Virtual-Key (Provider und/oder
-- Modelle). Leere/fehlende Liste = unbeschränkt in dieser Dimension;
-- beide gesetzt = UND.
-- Bewusst ON DELETE RESTRICT auf provider_id: ein Provider-Löschversuch
-- darf den Scope nicht still aufweiten (der Handler antwortet 409).
-- CASCADE auf virtual_key_id: Key-Löschung räumt die Scopes mit.
CREATE TABLE IF NOT EXISTS key_providers (
    virtual_key_id UUID NOT NULL REFERENCES virtual_keys(id) ON DELETE CASCADE,
    provider_id UUID NOT NULL REFERENCES providers(id) ON DELETE RESTRICT,
    PRIMARY KEY (virtual_key_id, provider_id)
);

CREATE TABLE IF NOT EXISTS key_models (
    virtual_key_id UUID NOT NULL REFERENCES virtual_keys(id) ON DELETE CASCADE,
    model_name TEXT NOT NULL,
    PRIMARY KEY (virtual_key_id, model_name)
);
