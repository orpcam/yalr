import type { Model } from "./api";

/** Modell-Scope-Option: Client-seitiger Modellname + liefernde Provider. */
export interface ModelOption {
  name: string;
  /** Provider-Instanznamen, die dieses Modell liefern (dedupliziert) */
  providers: string[];
}

/** GET /models liefert eine Zeile pro Provider-Instanz. Der Modell-Scope
 *  arbeitet mit dem Client-seitigen Modellnamen — hier deduplizieren. */
export function buildModelOptions(models: Model[]): ModelOption[] {
  const byName = new Map<string, Set<string>>();
  for (const m of models) {
    let set = byName.get(m.model_name);
    if (!set) {
      set = new Set();
      byName.set(m.model_name, set);
    }
    set.add(m.provider_name);
  }
  return [...byName.entries()]
    .map(([name, provs]) => ({ name, providers: [...provs] }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

/** Wert in Liste an- bzw. abwählen (neue Liste, keine Mutation). */
export const toggleValue = (list: string[], value: string): string[] =>
  list.includes(value) ? list.filter((v) => v !== value) : [...list, value];

/** Providernamen sind nicht unique — mit der kind disambiguieren */
export const providerLabel = (p: { name: string; kind: string }) =>
  `${p.name} (${p.kind})`;

/** Ausgewählte Werte, die zu keinen vorhandenen Optionen gehören (z.B.
 *  nachträglich gelöschte Modelle). Diese bleiben im Picker sichtbar. */
export const orphanValues = (selected: string[], optionValues: string[]): string[] => {
  const known = new Set(optionValues);
  return selected.filter((v) => !known.has(v));
};
