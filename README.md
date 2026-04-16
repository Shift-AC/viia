# viia

![logo](logo/logo.png)

A minimal TUI (based on `ratatui`)/GUI (based on `Tauri`) image viewer program that provides complete and customizable support of **v**iewing **i**mages **i**n **a**nimations.

## Why you need this (In other words, why I built this?)

Suppose you have a series of image files that represent a storyline, it is often attractive to concatenate them somehow into a continuous animation for you to view automatically. Typically, we realize such an idea with slideshows. However, slideshows are typically a minor feature of image viewers, and thus they often lack fine-grained control over the timing of each image, and support for various image formats --- especially for animated ones. Therefore, we have `viia` (View Images In Animations) here.

`viia` is an image viewer that focuses on animations/slideshows. As stated by its name, it treats all images as animations, and enables you to assemble stand-alone animations into larger ones for you to view. To achieve this, `viia` provides programmable support for controlling the timing of each image in the slideshow, either by controlling the time or loops that it should be played. Consider you have a series of images that composes different parts in a complete animation, some of them are a single loop that may be played for multiple times, and others are stand-alone scenes. With `viia`, you could specify the number of loops to play for loop scenes or simply let them to play for a specified period, while playing others for only once. During the playback, you could even pause/resume them. For static images (which is considered as animation with only one frame by `viia`), `viia` also properly handles the animation work by allowing you to show the images rich in details longer, while only briefly showing the others. Because the animated images are often large and require more time to load/decode, `viia` also applies proper caching to accelerate image loading, ensuring seamless playback experience.

Of course, in addition to slideshows, `viia` also supports anything that a basic image viewer would do, and a little more: 

1. Resizing animated images;
2. Viewing remote images over SFTP.

You may expect this list to grow in the future as I find out how to make `viia` easier to use :)

## Build/Usage

`viia` is originally built as a Windows program, but since it is using cross-platform GUI/TUI frameworks, theoretically it is also possible to build it on other platforms --- Linux/x86_64 seems to be working. No matter what platform you are on, you may build `viia` with: 

```
cargo build --release
```

The `viia` project mainly provides two programs, `viia` and `viiaw`. `viia` is the main image viewer program that supports all the UI modes, while `viiaw` is a specific fork of `viia`'s GUI mode, which does not associate itself with a terminal upon startup. If you would use `viia` in your terminal (either use the TUI directly or start a GUI window) or need the debug messages, use the `viia` program. If you are GUI-native and expect something like other image viewer applications, use `viiaw`.

## Basic idea

The `viia` program treats each image file as an animation (static images are simply animations with only one frame), and animations could be concatenated together to form a larger animation. In this definition, Display of every single image is just like playing a slideshow. `viia` constructs an image list according to the user input, applies parameters from either the user or some presets to control the timing of each image in each slideshow, and plays them accordingly.

## Calling interface

- Image list

    `viia` accepts a list of paths to image files or directories as its command line arguments. If a single file path is provided, `viia` will display that file first and then load all other image files in its parent directory (in lexicographical order). If directory paths are provided, `viia` will automatically expand them to include all files inside them (in lexicographical order). If multiple files are provided, `viia` will follow the order they are provided.

    No matter how the image list is constructed, `viia` will display the first image that corresponds to the first provided path at startup.

- Startup options

    `viia`'s command line interface is quite simple, with only a few options to define its startup state. The users are expected to further interact with `viia` through the runtime commands, as specified below:

    | Option | Description |
    |---------|-------------|
    | `-d`, `--dimension` | Window dimension ([width]x[height]), 2/3 of the screen size if not specified |
    | `--ui` | The user interface to use, `headless`, `terminal` or `gui` (default is `terminal`) |

    Here the `headless` mode is just like any normal terminal programs. It would not display any graphical output, but would still perform any other operations and generate log for debugging; the `terminal` mode is a terminal image viewer. It accepts user inputs through the terminal and renders images using **sixel** graphics. The `gui` mode launches a `Tauri` frontend for you to operate on.

- Runtime commands (internal shell)

    No matter which user interface is used, `viia` is backed by an internal shell that accepts internal commands and produces outputs accordingly. The users are expected to further interact with `viia` through the runtime commands, as specified below:

    | Command | Description |
    |---------|-------------|
    | `d [dim]` | Set window dimension (e.g., `d 800x600`). Omit the value to use screen size |
    | `g [index]`| Go to an image by file-list index. Omit index to show the current image |
    | `h`, `help` | Print help information |
    | `l`       | Show the previous image |
    | `m [pattern]` | Print file names in the current file list that match a regex pattern |
    | `o [targets...]` | Open a new set of files, directories, or URLs |
    | `p`       | Pause/Resume the current slideshow |
    | `q`       | Quit the program |
    | `r`       | Show the next image |
    | `s [cmd]` | Start a slideshow with a command string (see [Slideshow specification format](#slideshow-specification-format)) |
    | `z [mode]`| Set the zoom mode. Options: `fit` (scale to fit window), `shrink` (only shrink if too large), or a fixed scale percentage (e.g., `150`) |

    In its implementation, `viia` designed its TUI and GUI to be as *dumb* as possible: they do minimal processing other than handling the UI. The real display work is typically done by the internal shell. The UI simply take user operations, generate command lines for the internal shell to execute, and renders the outputs on the UI. Therefore, *no matter which user interface is used*, you may use the runtime commands to control the behavior of `viia`.
    
## Slideshow specification format

The core feature of `viia` that makes it unique is its ability to define a slideshow with customizable commands. The commands are written in plain text, as specified below:

- Timing for a single image

    __Syntax__: `L[loops]T[time_in_seconds]` or `INF`

    Here `L[loops]` specifies that the image should be played for `loops` times. `viia` treats static images as animated images with only one frame that lasts 100 ms. Thus `L[loops]` applies for both static and animated images. `T[time_in_seconds]`, on the other hand, specifies the _minimum_ time to play the image: after expiration of the time period, `viia` waits for a loop to end before advancing to the next image. You could provide either `L` or `T` limit for an image, if both are provided, `viia` picks the one that lasts longer.
    
    Alternatively, you can specify `INF` to loop the image eternally.

    We also note that `viia` removes all whitespaces from the command string and is case-insensitive so you could write things like `l1 T    2` or `Inf` and this would not pose difficulty for `viia` to understand your command. 

- Timing for multiple images

    __Syntax__: `[command],[command]`
    
    To specify the timing for multiple images, you could simply write a comma-separated list of commands. For example, `L1T2, L2T3, L3` specifies the timing for three images. `viia` uses the command list repeatedly, so for the fourth image, `viia` applies `L1T2`, and similar thing goes for remaining ones. It is convenient for you to simply write `Lx` to loop all images for `x` times.

- Read commands from a file

    __Syntax__: `@[filename]`

    To read commands from a file, you could simply write `@[filename]` in the command string. `viia` will read the file and use the commands in it. 
