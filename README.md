# Nocturne

**Tu música, convertida en mapa de ritmo.** Nocturne es un juego de ritmo de cuatro carriles escrito
en Rust (Bevy). Importas una canción, la analiza (tempo, golpes, compases, patrones) y genera
automáticamente cuatro mapas —Easy, Normal, Hard y Expert—. Después juegas, miras tus estadísticas y,
si un mapa no te convence, lo corriges en el editor.

* Pista limpia: cuatro círculos fijos y notas blancas lisas que caen hasta ellos. Sin líneas, sin letras.
* Análisis determinista: el mismo audio produce siempre el mismo mapa.
* Todo local: biblioteca en SQLite, mapas en JSON, ajustes en JSON. Sin red.

## Compilar y ejecutar

Hace falta Rust estable reciente (Bevy 0.20 exige 1.97.1 o superior).

```sh
cargo run --release -p nocturne
```

En Linux se necesitan las cabeceras de ALSA, udev, Wayland/X11 y xkbcommon. En Debian/Ubuntu:

```sh
sudo apt install libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev pkg-config
```

## Cómo se usa

1. **Biblioteca → Importar** (`I`): elige un archivo o una carpeta, o **arrastra** archivos a la ventana.
   Formatos: MP3, WAV, FLAC, OGG (Vorbis), M4A/AAC y la pista de audio de MP4/MOV. Se analizan en segundo plano
   (dos a la vez); puedes seguir navegando.
2. Elige canción (`↑↓`) y dificultad (`←→`) y pulsa `Intro`. Teclas por defecto: **D F J K**.
3. `Esc` pausa. Al terminar ves tu nota, la distribución de errores (¿pulsas pronto o tarde?) y se guarda en
   las estadísticas.
4. **Editar mapa** (`E`) abre el mapa en el editor.
5. **Reanalizar** (`R`) regenera los cuatro mapas (por ejemplo tras subir la sensibilidad en Ajustes).

### Editor de mapas

| | |
|---|---|
| Clic / arrastrar / clic derecho | poner nota / nota larga / borrar |
| Teclas de carril | nota en el cursor (otra vez la quita); con `Mayús`, inicio y fin de una larga |
| `↑ ↓`, rueda, `Ctrl+↑↓`, `RePág/AvPág` | mover el cursor: una subdivisión, un compás, cuatro compases |
| `Espacio` | reproducir; cada nota que cruza el cursor hace «tic» |
| `Tab` / `G` | subdivisión (1/4 … 1/32) / ajuste libre de 10 ms |
| `-` `=` | zoom |
| `,` `.` | desfase de toda la canción (`Mayús` ×10) |
| `Ctrl+Z`, `Ctrl+Y`, `Ctrl+S` | deshacer, rehacer, guardar |
| `T` | probar el mapa desde el cursor (modo práctica, no puntúa) |

Las notas añadidas a mano se ven en verde. Ninguna edición puede dejar un mapa inválido: los solapes
en un carril, las notas fuera de la canción y las notas largas demasiado cortas se rechazan con un mensaje.

### Ajustes

Cinco secciones, todo se guarda al instante: **Juego** (velocidad, tamaño de las notas, separación entre
carriles, borde y altura de los círculos fijos —con vista previa—, precisión exigida, sin fallo, HUD),
**Controles** (perfiles y remapeo de las cuatro teclas), **Audio** (volúmenes, dispositivo de salida,
compensación de audio y **calibración con metrónomo**), **Gráficos** y **Análisis**
(sensibilidad, notas largas, densidad). `Q`/`E` cambian de sección.

Si tus pulsaciones salen sistemáticamente tarde o pronto, las **Estadísticas** te sugieren la
compensación de audio, y la **Calibración** la calcula por ti.

## Línea de órdenes

```
nocturne --import <ruta>        analiza un archivo o una carpeta (repetible)
nocturne --play <archivo>       importa si hace falta y empieza a jugar (--difficulty easy|normal|hard|expert)
nocturne --edit <archivo>       abre el mapa en el editor
nocturne --screen <pantalla>    home | library | settings | stats | about | calibrate
nocturne --demo [--autoplay]    mapa de demostración sin audio
nocturne --probe-audio          mide el reloj del dispositivo de audio
nocturne --help
```

`NOCTURNE_HOME=<dir>` redirige configuración y datos (útil para pruebas). Por defecto se usan
`~/.config/nocturne` y `~/.local/share/nocturne` (respetando `XDG_*`).

`cargo run -p nocturne-core --bin nocturne-analyze -- <audio> [salida]` analiza un archivo sin interfaz
e imprime el diagnóstico (tempo, secciones, notas por dificultad).

## Estructura

```
crates/nocturne-core   lógica pura, sin ventana ni audio: decodificación, análisis, generador,
                       juicio y puntuación, editor de mapas. Se prueba con audio sintético.
crates/nocturne        la aplicación (Bevy): pantallas, audio (cpal), base de datos (SQLite), ajustes.
docs/BEATMAP_FORMAT.md formato del mapa (JSON versionado)
docs/AUDIO_ANALYSIS.md cómo se analiza el audio y cómo se generan los mapas
tools/run_on_laptop.sh script de verificación para Hyprland
```

Decisiones de diseño que conviene conocer:

* **Reloj de audio**: la posición de la música sale del reloj del dispositivo (con su latencia
  compensada) y se suaviza contra el reloj monotónico; la posición de cada nota se calcula a partir de
  él, nunca acumulando desplazamientos por fotograma, así que no depende de los FPS.
* **Sin dispositivo de audio** el juego sigue siendo utilizable con un reloj silencioso.
* **Robustez**: ajustes y base de datos corruptos se apartan (`.bak`, `.corrupt`) y se sustituyen con un
  aviso; los análisis interrumpidos se marcan como fallidos al arrancar; los guardados son atómicos.

## Pruebas

```sh
cargo test --workspace
```

## Licencia

GPL-3.0-or-later. Tipografía Space Grotesk bajo SIL Open Font License 1.1.
