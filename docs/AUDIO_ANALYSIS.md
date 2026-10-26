# Análisis de audio y generación de mapas

Todo el análisis está en `nocturne-core` (sin ventana ni dispositivo de audio) y se prueba con
audio sintético en `cargo test`. Es **determinista**: mismo audio, mismos parámetros y misma
versión producen exactamente el mismo mapa (no hay aleatoriedad; los desempates usan un hash de la
huella del audio y del instante).

```
archivo → decodificar → huella → mono → STFT/bandas → flujo espectral → candidatos de onset
        → tempo (+ secciones) → beats → cuadrícula/subdivisión → patrones por compás
        → segmentos de intensidad → eventos → generador (4 dificultades) → mapas JSON
```

## 1. Decodificación (`decode.rs`)

`symphonia` detecta el formato **por contenido**, no por extensión (la extensión es solo una pista).
Admite MP3, WAV/PCM, FLAC, OGG/Vorbis, AAC y M4A, y extrae la pista de audio de contenedores MP4/MOV.
Se rechazan con un mensaje claro los archivos sin pista de audio, corruptos, de menos de 2 s o de
más de 30 min (para acotar memoria). La decodificación puede cancelarse desde otro hilo.

## 2. Características espectrales (`spectral.rs`)

* STFT con ventana de Hann de ≈ 46 ms (potencia de 2) y salto de 1/8 de ventana (≈ 5,8 ms).
* Banco de 80 bandas triangulares de espaciado logarítmico desde 30 Hz; no se guarda el espectro lineal.
* Compresión logarítmica `ln(1 + 100·x)`.
* Se calculan además el RMS y el centroide espectral por fotograma, y tres zonas: graves (< 200 Hz),
  medios y agudos (> 2,5 kHz).

## 3. Onsets (`onset.rs`)

Flujo espectral (SuperFlux) → máximos locales → umbral adaptativo (media local de ±0,4 s más una
fracción de la referencia global, que depende de la *sensibilidad* del usuario) → supresión por
distancia mínima (45 ms) → ajuste fino sobre la señal temporal. Cada candidato conserva su
**evidencia** (fuerza 0..1, banda, nivel en dBFS) y, si se descarta, **por qué**
(`BelowThreshold`, `TooClose`, `Silent`). Un suelo absoluto evita tratar el ruido numérico como ritmo.

## 4. Tempo y beats (`tempo.rs`)

* **Tempo**: autocorrelación de la envolvente de onsets (sin media móvil de 1 s), con refuerzo por
  armónicos y un prior log-gaussiano centrado en 120 BPM, en el rango 60–200 BPM. Se conservan las
  hipótesis alternativas (BPM/2, ×2…) y una *confianza* heurística.
* **Secciones**: si el tempo cambia a lo largo de la pista se estima por tramos.
* **Beats**: programación dinámica (Ellis, 2007) por sección. Se calibra el sesgo entre el centro del
  fotograma y el ataque real (mediana) y se descartan pulsos forzados en intros y colas sin eventos.
* **Primer tiempo**: de las 4 fases posibles, la que acumula más energía grave.

Si no hay pulso estable se avisa y el mapa se apoya solo en los onsets.

## 5. Cuadrícula y patrones (`rhythm.rs`)

Cada evento se sitúa en la cuadrícula de beats y se etiqueta como `Beat`, `Eighth`, `Sixteenth`,
`Triplet` u `OffGrid` (más de 35 ms o 0,12 beats de la posición más cercana). Los compases se
describen con una firma de 48 posiciones (4 beats × 12, válida para semicorcheas y tresillos) y se
agrupan los compases iguales en **patrones**. Una segmentación por energía marca la intensidad de
cada sección (0 baja, 1 media, 2 alta).

## 6. Generación (`beatmap/generator.rs`)

Las cuatro dificultades comparten la misma **importancia** por evento
(`0,45·fuerza + 0,25·posición métrica + 0,15·banda + 0,15·regularidad`) y solo cambian las reglas,
así que cada nivel es una simplificación reconocible del siguiente.

| | Importancia mín. | Hueco mín. | Máx. notas/s | Subdivisiones | Acordes | Largas |
|---|---|---|---|---|---|---|
| Easy | 0,55 | 330 ms | 2,6 | pulso | no | no |
| Normal | 0,42 | 210 ms | 4,6 | pulso, corchea | no | ≥ 600 ms, ≤ 8 % |
| Hard | 0,30 | 130 ms | 7,0 | + semicorchea, tresillo | importancia ≥ 0,85 | ≥ 450 ms, ≤ 12 % |
| Expert | 0,20 | 85 ms | 10,5 | igual | importancia ≥ 0,70 | ≥ 400 ms, ≤ 15 % |

1. **Selección**: por importancia descendente, respetando hueco mínimo y densidad en ventanas de 1 s
   (multiplicada por el ajuste *densidad*). Las secciones suaves exigen algo más; las intensas, algo menos.
2. **Notas largas**: solo donde la energía se mantiene (≥ −9 dB del pico tras el transitorio) el tiempo
   mínimo, sin invadir el siguiente evento y nunca en la banda aguda.
3. **Carriles**: se reutiliza el carril aprendido para el mismo patrón y posición del compás (repetición
   musical); si no, se elige por coste (evitar repetir carril en notas rápidas, alternar manos, graves
   al centro y agudos a los lados). Los acordes (solo en Hard/Expert) se limitan a pulsos fuertes y
   separan los carriles al menos dos posiciones.
4. **Validación**: lo que quede fuera de rango se elimina y se anota en `repairs`.

## Qué es heurístico

La confianza del tempo, la fuerza de los eventos y la importancia son **puntuaciones heurísticas**,
no probabilidades calibradas. Por eso el editor de mapas existe: el análisis acierta en música con
percusión clara y puede equivocarse en música ambiental, voces sin ritmo o tempos muy variables.
