#!/usr/bin/env bash
# Builds the Linux packages from the release tar.gz that package-linux.sh made, so every package
# carries exactly the archive's files:
#
#   build-packages.sh [-o <folder>] [-n <nfpm>] [-a <appimagetool>] [-r <AppImage runtime>] <nightjar-desktop-<version>-linux-x86_64.tar.gz>
#
# Writes beside the archive (or into -o):
#   nightjar-desktop-<version>-linux-x86_64.deb and .rpm (nfpm): the archive in /opt/nightjar,
#     /usr/bin/nightjar, and the archive's share/ (the desktop entry, the icon in the hicolor sizes
#     and the AppStream data) in /usr/share;
#   nightjar-desktop-<version>-x86_64.AppImage (appimagetool with the type 2 runtime given, so
#     nothing is fetched while it builds; no "linux" in the name, as the AppImage catalog asks):
#     the archive plus libopus, and libva, libva-drm and libdrm as fallbacks AppRun uses only
#     where the system has none;
#   PKGBUILD and io.github.ramazankara.nightjar.yml: the AUR and Flatpak recipes of this
#     folder with the version and the archive's SHA-256 filled in. They are not published.
#
# Run it on the machine that built the archive (the release workflow's Ubuntu runner): the
# packages' dependencies are read off the binaries there. The .deb names the Debian/Ubuntu
# packages that own the libraries the binaries link (dpkg -S), the C library with the newest
# GLIBC_ symbol version used; the .rpm asks for the same libraries by soname
# (libva.so.2()(64bit)), which every RPM distribution resolves. The libraries SDL3 and the
# PyroWave SDK open at run time (display, audio, input, Vulkan) are recommendations: a desktop
# has them, and a missing one only turns that backend off. nfpm, appimagetool and the runtime
# come from -n, -a, -r or PATH ($NFPM, $APPIMAGETOOL, $APPIMAGE_RUNTIME).
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
out=
nfpm=${NFPM:-nfpm}
appimagetool=${APPIMAGETOOL:-appimagetool}
runtime=${APPIMAGE_RUNTIME:-}
while getopts "o:n:a:r:" opt; do
	case $opt in
		o) out=$OPTARG ;;
		n) nfpm=$OPTARG ;;
		a) appimagetool=$OPTARG ;;
		r) runtime=$OPTARG ;;
		*) exit 2 ;;
	esac
done
shift $((OPTIND - 1))
[[ $# == 1 ]] || { echo "usage: build-packages.sh [-o <folder>] [-n <nfpm>] [-a <appimagetool>] [-r <runtime>] <tar.gz>" >&2; exit 2; }
archive=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
name=$(basename "$archive" .tar.gz)
[[ $name =~ ^nightjar-desktop-(.+)-linux-x86_64$ ]] || { echo "error: $archive is not named nightjar-desktop-<version>-linux-x86_64.tar.gz" >&2; exit 2; }
version=${BASH_REMATCH[1]}
[[ -n $out ]] || out=$(dirname "$archive")
mkdir -p "$out"
out=$(cd "$out" && pwd)
command -v "$nfpm" >/dev/null || { echo "error: nfpm not found (-n)" >&2; exit 1; }
command -v "$appimagetool" >/dev/null || { echo "error: appimagetool not found (-a)" >&2; exit 1; }
[[ -s $runtime ]] || { echo "error: the AppImage type 2 runtime is missing (-r)" >&2; exit 1; }
app_id=io.github.ramazankara.nightjar

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tar -C "$work" -xzf "$archive"
stage=$work/$name
[[ -x $stage/nightjar ]] || { echo "error: $archive has no $name/nightjar" >&2; exit 1; }

# The libraries the binaries link that the archive does not carry, and the newest glibc symbol.
binaries=("$stage/nightjar")
while IFS= read -r -d '' lib; do binaries+=("$lib"); done < <(find "$stage" -name '*.so*' -type f -print0)
needed=$(for file in "${binaries[@]}"; do readelf -d "$file"; done |
	sed -n 's/.*(NEEDED).*Shared library: \[\(.*\)\].*/\1/p' | sort -u |
	while read -r lib; do [[ -e $stage/lib/$lib || -e $stage/$lib ]] || echo "$lib"; done)
glibc=$(for file in "${binaries[@]}"; do objdump -T "$file"; done | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -n1)
[[ -n $glibc ]] || { echo "error: no GLIBC_ symbol versions found in the binaries" >&2; exit 1; }
# Opened with dlopen by SDL3 (X11, Wayland, audio, udev, D-Bus) and by the PyroWave SDK (Vulkan).
# SDL does not start without a video and an audio driver: X11 (which Wayland desktops run too,
# as XWayland) and ALSA (under PulseAudio and PipeWire as well) are the ones every desktop has, so
# those are dependencies; the rest only add features.
dlopened_required="libX11.so.6 libXext.so.6 libasound.so.2"
dlopened="libvulkan.so.1 libEGL.so.1 libGL.so.1 libwayland-client.so.0 libwayland-egl.so.1 libwayland-cursor.so.0
libxkbcommon.so.0 libdecor-0.so.0 libXcursor.so.1 libXi.so.6 libXrandr.so.2 libXss.so.1 libXfixes.so.3
libpulse.so.0 libpipewire-0.3.so.0 libudev.so.1 libdbus-1.so.3"

# The Debian/Ubuntu package of an installed library, by soname.
deb_owner() {
	local path candidate
	path=$(ldconfig -p | sed -n "s|^[[:space:]]*$1 (libc6,x86-64) => ||p" | head -n1)
	[[ -n $path ]] || return 0
	# dpkg knows a file by the path its package ships (/usr/lib or /lib, before or after the
	# merged /usr): try the name ldconfig gives, then the resolved one, then the /usr form.
	for candidate in "$path" "$(readlink -f "$path")" "/usr${path#/usr}"; do
		dpkg -S "$candidate" 2>/dev/null | head -n1 | cut -d: -f1 | grep . && return 0
	done
	return 0
}
# A YAML list item per argument.
yaml_list() { local item; for item in "$@"; do printf '    - "%s"\n' "$item"; done; }

deb_depends=("libc6 (>= $glibc)")
rpm_depends=("libc.so.6(GLIBC_$glibc)(64bit)")
for lib in $needed; do
	[[ $lib =~ ^(libc|libm|libdl|libpthread|librt)\.so ]] && continue
	[[ $lib == ld-linux* ]] && continue
	owner=$(deb_owner "$lib")
	[[ -n $owner ]] || { echo "error: no Debian package owns $lib, which the package links; install it on the build machine" >&2; exit 1; }
	deb_depends+=("$owner")
	rpm_depends+=("$lib()(64bit)")
done
for lib in $dlopened_required; do
	owner=$(deb_owner "$lib")
	[[ -n $owner ]] || { echo "error: no Debian package owns $lib, which SDL needs to start; install it on the build machine" >&2; exit 1; }
	deb_depends+=("$owner")
	rpm_depends+=("$lib()(64bit)")
done
deb_recommends=()
rpm_recommends=()
for lib in $dlopened; do
	owner=$(deb_owner "$lib")
	[[ -z $owner ]] || deb_recommends+=("$owner")
	rpm_recommends+=("$lib()(64bit)")
done
mapfile -t deb_depends < <(printf '%s\n' "${deb_depends[@]}" | sort -u)
mapfile -t deb_recommends < <(printf '%s\n' "${deb_recommends[@]}" | sort -u)
# Hardware decoding needs the GPU's VA-API driver, which nothing links.
deb_recommends+=("mesa-va-drivers | va-driver")
rpm_recommends+=("mesa-va-drivers")
# Forwarding webcams and audio devices runs the system's usbip tools through pkexec.
deb_recommends+=("usbip | linux-tools-generic" "pkexec | policykit-1")
rpm_recommends+=("usbip" "polkit")

# The archive's share/ (desktop entry, icons, AppStream data) goes to /usr/share, not /opt.
share=$work/share
[[ -s $stage/share/applications/$app_id.desktop ]] || { echo "error: $archive has no share/applications/$app_id.desktop; build it with this checkout's package-linux.sh" >&2; exit 1; }
mv "$stage/share" "$share"
# USB forwarding's helper, policy and udev rule go where polkit and udev look, not to /opt.
usb=$work/usb
[[ -x $stage/usb/nightjar-usb-helper ]] || { echo "error: $archive has no usb/nightjar-usb-helper; build it with this checkout's package-linux.sh" >&2; exit 1; }
mv "$stage/usb" "$usb"
printf '#!/bin/sh
# Loads the USB forwarding rule; a system without udev running has nothing to load.
udevadm control --reload 2>/dev/null || true
' >"$work/udev-reload.sh"

# One entry per file: the folders under /usr/share belong to the system's packages.
share_files() {
	local file
	(cd "$share" && find . -type f | sort) | while read -r file; do
		printf '  - src: %s\n    dst: /usr/share/%s\n    file_info:\n      mode: 0644\n' "$share/${file#./}" "${file#./}"
	done
}

# nfpm takes semver: 4.0.0-rc.1 becomes 4.0.0~rc.1 in the .deb and 4.0.0~rc.1 in the .rpm.
cat >"$work/nfpm.yaml" <<EOF
name: nightjar
arch: amd64
platform: linux
version: "$version"
version_schema: semver
maintainer: "RamazanKara <github@r-kara.de>"
vendor: RamazanKara
homepage: https://github.com/RamazanKara/nightjar
license: GPL-3.0-only
description: |-
  Moonlight-protocol game-streaming client
  Nightjar streams games from a PC running Sunshine, Apollo, Vibepollo, Rubylight or
  GeForce Experience, with hardware decoding, HDR10, VRR pacing, audio, keyboard, mouse and
  controllers. FFmpeg's decoders and dav1d are carried in /opt/nightjar/lib.
section: games
priority: optional
contents:
  - src: $stage/
    dst: /opt/nightjar
    type: tree
  - src: /opt/nightjar/nightjar
    dst: /usr/bin/nightjar
    type: symlink
$(share_files)
  - src: $usb/nightjar-usb-helper
    dst: /usr/lib/nightjar/nightjar-usb-helper
    file_info:
      mode: 0755
  - src: $usb/$app_id.usb.policy
    dst: /usr/share/polkit-1/actions/$app_id.usb.policy
    file_info:
      mode: 0644
  - src: $usb/90-nightjar-usb.rules
    dst: /usr/lib/udev/rules.d/90-nightjar-usb.rules
    file_info:
      mode: 0644
scripts:
  postinstall: $work/udev-reload.sh
  postremove: $work/udev-reload.sh
overrides:
  deb:
    depends:
$(yaml_list "${deb_depends[@]}")
    recommends:
$(yaml_list "${deb_recommends[@]}")
  rpm:
    depends:
$(yaml_list "${rpm_depends[@]}")
    recommends:
$(yaml_list "${rpm_recommends[@]}")
rpm:
  summary: Moonlight-protocol game-streaming client
  group: Amusements/Games
EOF
"$nfpm" package --config "$work/nfpm.yaml" --packager deb --target "$out/$name.deb"
"$nfpm" package --config "$work/nfpm.yaml" --packager rpm --target "$out/$name.rpm"

# AppImage: the archive as the AppDir, started by AppRun (the executable's run path $ORIGIN/lib
# and the PyroWave SDK beside it resolve inside the mounted image).
appdir=$work/AppDir
appimage=$out/nightjar-desktop-$version-x86_64.AppImage
cp -a "$stage" "$appdir"
install -m 755 "$here/AppRun" "$appdir/AppRun"
cp "$share/applications/$app_id.desktop" "$appdir/"
cp "$share/icons/hicolor/256x256/apps/$app_id.png" "$appdir/"
ln -s "$app_id.png" "$appdir/.DirIcon"
mkdir "$appdir/usr"
cp -r "$share" "$appdir/usr/share"
# An installed library's path, by soname.
lib_path() { ldconfig -p | sed -n "s|^[[:space:]]*${1//./\\.} (libc6,x86-64) => ||p" | head -n1; }
# libopus keeps one soname everywhere and has no plugins: carrying it lets the AppImage start
# where it is not installed. libva and libdrm go to lib/fallback, which AppRun puts on the library
# path only where the system has no copy: the system's libva finds its VA-API drivers, and Mesa
# needs the system's libdrm.
for lib in libopus.so.0:lib libva.so.2:lib/fallback/libva libva-drm.so.2:lib/fallback/libva libdrm.so.2:lib/fallback/libdrm; do
	path=$(lib_path "${lib%%:*}")
	[[ -n $path ]] || { echo "error: ${lib%%:*} is not installed on this machine" >&2; exit 1; }
	mkdir -p "$appdir/${lib#*:}"
	cp -L "$path" "$appdir/${lib#*:}/${lib%%:*}"
done
ARCH=x86_64 "$appimagetool" --no-appstream --runtime-file "$runtime" "$appdir" "$appimage" >"$work/appimagetool.log" 2>&1 ||
	{ cat "$work/appimagetool.log" >&2; exit 1; }
chmod +x "$appimage"

# The AUR and Flatpak recipes with this archive's version and hash.
sha=$(sha256sum "$archive" | cut -d' ' -f1)
pkgver=${version//-/_}
sed -e "s/@VERSION@/$version/g" -e "s/@PKGVER@/$pkgver/g" -e "s/@SHA256@/$sha/g" "$here/aur/PKGBUILD.in" >"$out/PKGBUILD"
sed -e "s/@VERSION@/$version/g" -e "s/@SHA256@/$sha/g" "$here/flatpak/$app_id.yml.in" >"$out/$app_id.yml"

echo "deb depends: ${deb_depends[*]}"
echo "rpm depends: ${rpm_depends[*]}"
for file in "$out/$name.deb" "$out/$name.rpm" "$appimage"; do
	echo "$file: $(du -h "$file" | cut -f1)"
done
