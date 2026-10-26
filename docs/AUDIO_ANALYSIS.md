# Análisis de audio y generación de mapas

Todo el análisis está en `nocturne-core` (sin ventana ni dispositivo de audio) y se prueba con
audio sintético en `cargo test`. Es **determinista**: mismo audio, mismos parámetros y misma
versión producen exactamente el mismo mapa (no hay aleatoriedad; los desempates usan un hash de la
huella del audio y del instante).

```
archivo → decodificar → huella → mono → STFT/bandas → flujo espectral → candidatos de onset
        → tempo (+ secciones) → beats → cuadrícula/subdivisión → patrones por compás
        → segmentos de intensidad → eventos → generador (6 dificultades) → mapas JSON
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

Las seis dificultades comparten la misma **importancia** por evento
(`0,45·fuerza + 0,25·posición métrica + 0,15·banda + 0,15·regularidad`) y solo cambian las reglas,
así que cada nivel es una simplificación reconocible del siguiente.

| | Importancia mín. | Hueco mín. | Eventos/s máx. | Débiles | Acordes | Largas | Relleno |
|---|---|---|---|---|---|---|---|
| Easy | 0,55 | 330 ms | 2,6 | no | no | no | no |
| Normal | 0,42 | 210 ms | 4,6 | no | no | ≥ 600 ms, ≤ 8 % | no |
| Hard | 0,28 | 120 ms | 7,5 | pulso y corchea | imp. ≥ 0,80, solo tiempo fuerte | ≥ 450 ms, ≤ 12 % | no |
| Expert | 0,18 | 80 ms | 11 | cuadrícula | imp. ≥ 0,62 (pulso, corchea) | ≥ 400 ms, ≤ 15 % | no |
| Master | 0,12 | 62 ms | 15 | cuadrícula | imp. ≥ 0,45 + manos de 3 (≥ 0,90) | ≥ 350 ms, ≤ 18 % | semicorcheas, huecos ≤ 1 pulso |
| Extreme | 0,06 | 48 ms | 20 | cuadrícula | imp. ≥ 0,30, carriles contiguos + manos de 3 (≥ 0,75) | ≥ 300 ms, ≤ 22 % | semicorcheas, huecos ≤ 2 pulsos, salto cada 2 notas |

1. **Selección**: primero los golpes aceptados por importancia descendente, y después los *débiles*,
   para que estos nunca desplacen a aquellos. Se respeta el hueco mínimo y la densidad: **en cualquier
   ventana de 1 s** hay como mucho `eventos/s × ajuste de densidad` eventos (los acordes pueden añadir
   hasta un 30 % más). Las secciones suaves exigen algo más; las intensas, algo menos.
2. **Golpes débiles** (Hard en adelante): el detector marca como «bajo el umbral» los golpes mucho más
   suaves que los más fuertes de la canción (hi-hats, shakers, arpegios). Se admiten solo si caen sobre una
   subdivisión de la cuadrícula —que encajen en ella es la prueba de que son reales— y superan una fuerza mínima.
   Master y Extreme admiten además golpes aceptados **fuera de cuadrícula** (swing, tempo mal estimado).
3. **Relleno de cuadrícula** (Master y Extreme): semicorcheas que puentean los huecos entre notas ya elegidas,
   solo donde ya hay actividad (≥ 2,5 o 1,5 notas/s en ±1,5 s) y fuera de silencios. **Es la única regla que no
   parte de un golpe medido** y por eso solo se aplica si la cuadrícula explica el audio: confianza del tempo
   ≥ 0,4 y al menos el 80 % de los golpes sobre la cuadrícula. Con un tempo dudoso se desactiva y el generador lo
   anota en `repairs`.
4. **Notas largas**: solo donde la energía se mantiene (≥ −9 dB del pico tras el transitorio) el tiempo
   mínimo, sin invadir el siguiente evento y nunca en la banda aguda.
5. **Carriles**: se reutiliza el carril aprendido para el mismo patrón y posición del compás (repetición
   musical); si no, se elige por coste. Se evita repetir un carril usado en el evento anterior (un acorde ocupa
   varios), se alterna de mano, y en pasajes rápidos se prefieren escaleras y trinos (carriles contiguos, misma
   dirección). Los acordes separan los carriles al menos dos posiciones (uno en Extreme) y las manos de tres solo
   salen en pulsos fuertes.
6. **Validación**: lo que quede fuera de rango se elimina y se anota en `repairs`.

El ajuste **Densidad** (Análisis) multiplica el tope de eventos por segundo de todos los niveles, de 0,5× a 3×.
Más allá de la rejilla de semicorcheas de la canción no se inventan más notas: a 128 BPM, Extreme ronda las
14–15 notas/s con picos de 19; en una canción de 180 BPM, las semicorcheas por sí solas ya son 12 notas/s.

## Qué es heurístico

La confianza del tempo, la fuerza de los eventos y la importancia son **puntuaciones heurísticas**,
no probabilidades calibradas. Por eso el editor de mapas existe: el análisis acierta en música con
percusión clara y puede equivocarse en música ambiental, voces sin ritmo o tempos muy variables.
