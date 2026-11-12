# Wica

Whats it called again?

Find the tool you installed a year ago with a simple query on what the tool
does.

![wica demo](asset/demo.gif)

## Why

While procrastinating I scroll through reddit and see all the cool cli tools.
I'm like I need this, I need that. Now I have 100 of them. But once a year when
I do real work I'm like: I had this tool, whats it called again?

## Install

### Nix

Try it without installing:

```shell
nix run github:couchpotato007/wica -- render markdown
```

Or with Home Manager:

```nix
# flake.nix inputs
wica.url = "github:couchpotato007/wica";

# home.nix
imports = [ inputs.wica.homeManagerModules.default ];

programs.wica = {
  enable = true;
  autoUpdate.enable = true; # runs `wica update` weekly
};
```

### Cargo

```shell
cargo install --git https://github.com/couchpotato007/wica
```

## Usage

On first run you should update the sources with `wica update`. This downloads
the tldr pages (~3 MB) and a small embedding model (~130 MB) and indexes all
your tools. The first time this takes quite a while and uses a lot of CPU.
After that only new tools get indexed, so it's fast.

```shell
wica render markdown in terminal   # describe what you want to do
wica name deer                     # you only remember part of the name
wica tldr gcc                      # all examples for a tool
```

### Describe what you want

```shell
wica edit video
wica pretty print json
wica disk usage
```

wica shows the tools you have installed first, each with an example. If a tool
you **don't** have fits a lot better, it shows that too, with the install
command for your distro:

```
Not installed, but a better fit:

 glow  Render Markdown in the terminal. (not installed)
       install: nix profile install nixpkgs#glow
       try:     nix shell nixpkgs#glow
```

### Half remember the name

```shell
wica name deer      # tealdeer
wica name ripgrap   # rg (ripgrep), typos are fine
wica n imagemagic   # magick
```

This also searches the packages of your package manager, so it finds things
you never installed. 

### Included tldr

```shell
wica tldr tar
```

Shows the whole [tldr](https://tldr.sh) page, offline. So you don't need a
separate tldr client (but tealdeer is great).

### Teach it your own words

```shell
wica tag rg grep search text
```

Now `wica grep` also finds `rg`. Tags are stored in `~/.config/wica/tags`.

## How does it work

wica looks at everything on your `$PATH`, reads the man pages and the tldr
pages, and searches them in two ways:

- by **keywords**, like a normal search engine
- by **meaning**, with a tiny embedding model
  ([bge-small](https://huggingface.co/BAAI/bge-small-en-v1.5)). That's why
  "render markdown" finds glow and not blender, and "edit video" finds kdenlive.

The model runs locally with [candle](https://github.com/huggingface/candle).
Your queries never leave your machine. Everything is cached in `~/.cache/wica`.

## Package managers

To show install commands, wica asks your package manager which package has the
command (`rg` is in `ripgrep`). Supported:

| Manager | One-time setup for package lookup                             |
| ------- | ------------------------------------------------------------- |
| nix     | `nix-index` (or use nix-index-database)                       |
| pacman  | `sudo pacman -Fy`                                             |
| apt     | `apt-file` (`sudo apt install apt-file && sudo apt-file update`) or Ubuntu's command-not-found |

Without the setup wica still shows an install command, but it guesses the
package name and tells you so.

`wica managers` shows what was detected.

> Honest note: so far only nix has really been tested. If you use another
> distro, please try it and open an issue if something is off.

## Configuration

| What                       | Where                                     |
| -------------------------- | ----------------------------------------- |
| Your tags                  | `~/.config/wica/tags` (use `wica tag`)    |
| Extra package managers     | `~/.config/wica/package-managers.toml`    |
| Embedding model            | `WICA_MODEL` environment variable         |

If your laptop is a potato, use a smaller and much faster model. It's a bit
dumber though:

```shell
export WICA_MODEL=minishlab/potion-base-8M
wica update
```

## Limitations

- English only.
- The first `wica update` takes a while.
- wica only knows tools that are installed, have a man page or have a tldr page.

## Thanks

- [tldr-pages](https://github.com/tldr-pages/tldr) for the descriptions and
  examples. wica would be useless without them.
- [BAAI](https://huggingface.co/BAAI) for the bge model,
  [candle](https://github.com/huggingface/candle) and
  [model2vec](https://github.com/MinishLab/model2vec-rs) for running models in
  Rust.

## AI Usage

Yes. Most of the code was written together with Claude.

## License

[MIT](LICENSE)
