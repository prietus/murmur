# Arch Linux package

Source-based PKGBUILD that builds Murmur from the tagged release.

## Build locally

```sh
cd packaging/arch
makepkg -si
```

## Submit to AUR

1. Update `pkgver` to match the latest tag.
2. Compute the source checksum:
   ```sh
   updpkgsums
   ```
   (or replace `SKIP` manually with the output of
   `curl -L https://github.com/prietus/murmur/archive/refs/tags/v0.1.0.tar.gz | sha256sum`)
3. Generate `.SRCINFO`:
   ```sh
   makepkg --printsrcinfo > .SRCINFO
   ```
4. Push to the AUR git remote:
   ```sh
   git clone ssh://aur@aur.archlinux.org/murmur.git aur-murmur
   cp PKGBUILD .SRCINFO aur-murmur/
   cd aur-murmur && git add . && git commit -m "v0.1.0" && git push
   ```

## Variants you can add later

- `murmur-bin` — installs the GitHub Releases binary, no compile.
- `murmur-git` — builds from `master`, version derived from `git describe`.
