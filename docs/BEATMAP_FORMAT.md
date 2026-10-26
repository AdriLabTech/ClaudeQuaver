# Formato de mapa (`format_version` 1)

Cada canción tiene **cuatro mapas**, uno por dificultad, guardados como JSON legible en el
directorio de datos (`maps/<huella>.<dificultad>.json`, p. ej. `4928ef5b2d3e462a.hard.json`).
La definición está en `crates/nocturne-core/src/beatmap/mod.rs`; este documento la describe.

El audio **no** forma parte del mapa: la biblioteca guarda solo su ruta y la huella. El mapa se
une a su audio por `audio_fingerprint`.

## Campos

| Campo | Tipo | Significado |
|---|---|---|
| `format_version` | entero | Versión del formato. Un mapa con otra versión se rechaza con un error claro. |
| `generator_version` | texto | Versión del generador y del analizador (`"1+1"`); `"demo"` para el mapa de demostración. |
| `title`, `artist` | texto | Metadatos. El título sale del nombre del archivo; el artista queda vacío si no se conoce. |
| `audio_fingerprint` | texto | FNV-1a de 64 bits (hex) de las muestras decodificadas, cuantizadas a 16 bits. Mismo audio → misma huella, aunque cambie el contenedor. |
| `duration` | µs | Duración del audio. |
| `music` | objeto | `bpm` (opcional), `bpm_confidence` (0..1), `bpm_candidates` (`[[bpm, puntuación]]`, hipótesis alternativas como BPM/2 o ×2) y `first_beat` (µs, opcional). |
| `difficulty` | `Easy` \| `Normal` \| `Hard` \| `Expert` | Nivel. |
| `parameters` | texto | Parámetros del generador legibles (`holds=true density_scale=1.00`). |
| `notes` | lista | Ver más abajo. |
| `song_offset_ms` | número | Desfase propio de la canción (ms). **Positivo = las notas llegan más tarde.** Se suma al ajuste global del jugador. Opcional; por defecto 0. |
| `edited_manually` | booleano | `true` si el mapa pasó por el editor. Reanalizar la canción lo sobrescribe (la biblioteca pide confirmación). |
| `repairs` | lista de textos | Eventos descartados por el generador (por ejemplo «sin carril libre»). |

Todos los instantes son **enteros en microsegundos** desde el inicio del audio (`Micros`). Se
evita la coma flotante para que un mapa guardado y releído sea idéntico.

### Notas

```json
{ "id": 12, "time": 18452000, "lane": 2, "kind": "Tap",
  "confidence": 0.81, "pattern": 3, "manual": false }

{ "id": 13, "time": 20000000, "lane": 0,
  "kind": { "Hold": { "end": 21250000 } },
  "confidence": 0.9, "pattern": null, "manual": true }
```

* `lane`: 0‥3 (de izquierda a derecha).
* `kind`: `"Tap"` o `{"Hold": {"end": µs}}` (nota larga: se pulsa en `time` y se suelta en `end`).
* `confidence`: heurística 0..1 del evento de origen (no es una probabilidad calibrada).
* `pattern`: identificador del patrón de compás al que pertenece el evento, si se reconoció.
* `manual`: `true` si la nota se añadió o movió a mano en el editor.
* `id`: estable dentro del mapa y **nunca reutilizado** al borrar.

## Invariantes (`Beatmap::validate`)

Al cargar un mapa se comprueba todo esto; si falla, el mapa se rechaza (no se «repara» en silencio):

1. `lane < 4`.
2. `time ≥ 0` y `end ≤ duration`.
3. `id` único.
4. Un hold termina después de empezar.
5. En un mismo carril, cada nota empieza **estrictamente después** de que termine la anterior.

El editor (`nocturne_core::editor`) va más allá y exige un hueco mínimo de 30 ms entre notas del
mismo carril y 120 ms de duración mínima para una nota larga; ninguna edición puede dejar un mapa
inválido.

## Evolución

`format_version` solo sube con cambios incompatibles. Los campos opcionales con valor por defecto
(`song_offset_ms`, `edited_manually`, `repairs`, `manual`) pueden añadirse sin subir la versión.
