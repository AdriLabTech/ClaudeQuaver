# Nocturne

**Tu música, convertida en mapa de ritmo.** Nocturne es un juego de ritmo de cuatro carriles escrito
en Rust (Bevy). Importas una canción, la analiza (tempo, golpes, compases, patrones) y genera
automáticamente seis mapas —Easy, Normal, Hard, Expert, Master y Extreme—. Después juegas, miras tus estadísticas y,
si un mapa no te convence, lo corriges en el editor.

* Pista limpia: cuatro círculos fijos y notas blancas lisas que caen hasta ellos. Sin líneas, sin letras.
* Análisis determinista: el mismo audio produce siempre el mismo mapa.
* Seis dificultades, de Easy (~1 nota/s) a **Extreme** (más de 14 notas/s sostenidas, con saltos, manos de tres
  notas y *streams*), y un **nivel numérico** por mapa para compararlos entre canciones.
* Todo local: biblioteca en SQLite, mapas en JSON, ajustes en JSON. Sin red.

## Instalación

Hace falta Rust estable reciente (Bevy 0.20 exige **1.97.1 o superior**) y un driver Vulkan (Bevy
renderiza con wgpu). El audio sale por ALSA, que en un sistema con PipeWire pasa por `pipewire-alsa`.

### Arch Linux y Ryoku Arch

Ryoku es Arch con `pacman` y escritorio Wayland (Hyprland o niri), así que la guía es la misma. Arch no
tiene paquetes `-dev`: las cabeceras vienen con cada librería. `--needed` salta lo que ya tengas.

**1. Dependencias**

```sh
sudo pacman -S --needed base-devel git rust pkgconf alsa-lib systemd-libs wayland \
    libxkbcommon libxkbcommon-x11 libx11 libxcursor libxi libxrandr vulkan-icd-loader
```

* **Rust**: el paquete `rust` de Arch suele estar al día; comprueba `rustc --version` (≥ 1.97.1). Si prefieres
  `rustup` (entra en conflicto con `rust`, así que quita `rust` del comando): `rustup default stable && rustup update`.
* **Driver Vulkan** según tu GPU: AMD `vulkan-radeon`, Intel `vulkan-intel`, NVIDIA `nvidia-utils` (con el driver
  propietario). Ryoku detecta tu GPU al instalarse y normalmente ya lo trae; compruébalo con
  `vulkaninfo --summary` (paquete `vulkan-tools`).
* **Audio**: `sudo pacman -S --needed pipewire pipewire-alsa pipewire-pulse wireplumber` si aún no usas PipeWire.

**2. Compilar**

```sh
git clone https://github.com/AdriLabTech/ClaudeQuaver.git nocturne
cd nocturne
cargo build --release --locked -p nocturne
```

La primera compilación tarda varios minutos (Bevy). Para probarlo sin instalar: `./target/release/nocturne`.

**3. Instalar** (opcional; deja `nocturne` en el `PATH` y una entrada en el lanzador de aplicaciones)

```sh
sudo install -Dm755 target/release/nocturne /usr/local/bin/nocturne
sudo install -Dm644 packaging/nocturne.desktop /usr/local/share/applications/nocturne.desktop
```

* **Actualizar**: `git pull`, vuelve a compilar y repite los dos `install`.
* **Desinstalar**: `sudo rm /usr/local/bin/nocturne /usr/local/share/applications/nocturne.desktop`.
  Tus datos quedan en `~/.config/nocturne` y `~/.local/share/nocturne`; bórralos si quieres empezar de cero.

**En Hyprland o niri**: la ventana usa el `app_id` (clase) `nocturne`, por si quieres una regla de ventana para
que se abra flotante o a pantalla completa (mira la sintaxis en la documentación de tu compositor). `F11`
alterna la pantalla completa dentro del juego.

### Debian y Ubuntu

```sh
sudo apt install build-essential pkg-config libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev \
    libxkbcommon-x11-0 libx11-dev libxcursor-dev libxi-dev libxrandr-dev libvulkan1 mesa-vulkan-drivers
cargo build --release --locked -p nocturne
```

Para probar sin instalar nada en cualquier distribución: `cargo run --release -p nocturne`.

### Solución de problemas

| Síntoma | Causa y arreglo |
|---|---|
| `rustc 1.9x is not supported by the following packages: bevy…` | Rust demasiado antiguo: `sudo pacman -Syu rust` (o `rustup update`). |
| `The system library alsa required by crate alsa-sys was not found` | Falta `alsa-lib` (y `pkgconf`). |
| `wayland-client.pc` / `xkbcommon` / `x11.pc` no encontrados | Faltan `wayland`, `libxkbcommon`, `libx11`: instala la lista del paso 1. |
| `Library libxkbcommon-x11.so could not be loaded` al arrancar | Falta `libxkbcommon-x11`. |
| Ventana negra o error de wgpu («no adapter») | Falta el driver Vulkan de tu GPU (arriba). Prueba `WGPU_BACKEND=vulkan nocturne`. |
| Sin sonido, o aviso «Audio no disponible» | Instala `pipewire-alsa`. `nocturne --probe-audio` lista las salidas y mide la latencia; elige otra en Ajustes → Audio. |
| Error de compilación de SQLite (`cc`) | Falta un compilador de C: instala `base-devel`. |

## Cómo se usa

1. **Biblioteca → Importar** (`I`): elige un archivo o una carpeta, o **arrastra** archivos a la ventana.
   Formatos: MP3, WAV, FLAC, OGG (Vorbis), M4A/AAC y la pista de audio de MP4/MOV. Se analizan en segundo plano
   (dos a la vez); puedes seguir navegando.
2. Elige canción (`↑↓`) y dificultad (`←→`) y pulsa `Intro`. Teclas por defecto: **D F J K**.
3. `Esc` pausa. Al terminar ves tu nota, la distribución de errores (¿pulsas pronto o tarde?) y se guarda en
   las estadísticas.
4. **Editar mapa** (`E`) abre el mapa en el editor.
5. **Reanalizar** (`R`) regenera los seis mapas (por ejemplo tras subir la sensibilidad en Ajustes, o para crear
   los niveles Master y Extreme en canciones importadas con una versión anterior).

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
carriles, borde y altura de los círculos fijos —con vista previa—, **desactivar efectos visuales**,
precisión exigida, sin fallo, HUD),
**Controles** (perfiles y remapeo de las cuatro teclas), **Audio** (volúmenes, dispositivo de salida,
compensación de audio y **calibración con metrónomo**), **Gráficos** y **Análisis**
(sensibilidad, notas largas, densidad). `Q`/`E` cambian de sección.

**Desactivar efectos visuales** (también en Gráficos) es el modo competitivo: quita los anillos al acertar,
el resalte de los círculos fijos al pulsar, los desvanecidos y las animaciones. Juicios, combo, vida y
progreso siguen mostrándose según sus propios ajustes.

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
packaging/             entrada de escritorio (.desktop) para el lanzador de aplicaciones
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
