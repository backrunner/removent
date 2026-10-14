#!/bin/bash
# Shared by the bundle launcher, SSH installer and local installer. Never sudo:
# a user-owned command directory works even when /usr/local/bin is read-only.
set -euo pipefail
umask 022

fail() { printf 'removent: %s\n' "$*" >&2; exit 1; }
[ "$#" -ge 1 ] && [ "$#" -le 2 ] || fail 'Usage: install-cli.sh /path/to/Removent.app [bin-directory]'
[ -n "${HOME:-}" ] && [ -d "$HOME" ] || fail 'HOME must be an existing directory'
case "$1" in /*) ;; *) fail 'Application path must be absolute' ;; esac
app=$(cd "$1" && pwd -P)
cli="$app/Contents/MacOS/removent-cli"
[ -f "$cli" ] && [ -x "$cli" ] || fail "CLI is missing: $cli"
bin=${2:-}
if [ -z "$bin" ]; then
    if [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then bin=/usr/local/bin
    else bin="$HOME/.local/bin"; fi
fi
case "$bin" in /*) ;; *) fail 'Command directory must be absolute' ;; esac
case "$bin" in *:*) fail 'Command directory must not contain a colon' ;; esac
case "$app$bin$HOME${ZDOTDIR:-}${XDG_CONFIG_HOME:-}" in *$'\n'*|*$'\r'*) fail 'Paths must not contain newlines' ;; esac
mkdir -p "$bin"
bin=$(cd "$bin" && pwd -P)

# Only repair our links. Never replace a file, directory or another tool's link.
owned_link() {
    [ -L "$1" ] || return 1
    local target
    target=$(readlink "$1")
    [ "$target" = "$cli" ] && return 0
    case "$target" in /*/Removent.app/Contents/MacOS/removent-cli) return 0 ;; esac
    return 1
}
for name in removent removent-cli; do
    path="$bin/$name"
    if { [ -e "$path" ] || [ -L "$path" ]; } && ! owned_link "$path"; then
        fail "Existing command preserved: $path. Move it before installing the Removent commands."
    fi
    existing=$(command -v "$name" || true)
    if [ -n "$existing" ] && [ "$existing" != "$path" ] && ! owned_link "$existing"; then
        fail "Existing command preserved: $existing. Resolve the name conflict before installing the Removent commands."
    fi
done

# A lock also prevents two bundle launches from appending profile entries twice.
lock="$bin/.removent-cli-install.lock"
mkdir "$lock" 2>/dev/null || fail "CLI setup is already running, or a stale lock remains: $lock"
temporary=
cleanup() {
    [ -z "$temporary" ] || rm -f "$temporary"
    rmdir "$lock"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Single-quote paths instead of interpreting shell metacharacters in home/app names.
quote() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
quoted_bin=$(quote "$bin")
profile_files=()
profile_blocks=()
profile_count=0
add_profile() {
    local file=$1 block=$2
    # Follow existing dotfile symlinks, retaining the user's dotfile management.
    [ ! -e "$file" ] || [ -f "$file" ] || fail "Shell profile is not a file: $file"
    if [ -L "$file" ] && [ ! -e "$file" ]; then fail "Shell profile is a broken link: $file"; fi
    if [ -f "$file" ] && grep -Fqx '# Removent command line' "$file"; then
        # The managed PATH file changes when the command directory moves.
        return
    fi
    [ ! -e "$file" ] || [ -w "$file" ] || fail "Shell profile is not writable: $file"
    profile_files+=("$file")
    profile_blocks+=("$block")
    profile_count=$((profile_count + 1))
}

# /usr/local/bin is part of macOS's standard login PATH. User/custom directories
# need shell startup integration, including interactive non-login shells.
if [ "$bin" != /usr/local/bin ]; then
    config="$HOME/.config/removent"
    mkdir -p "$config"
    path_file="$config/path.sh"
    fish_path_file="$config/path.fish"
    for file in "$path_file" "$fish_path_file"; do
        if [ -e "$file" ] || [ -L "$file" ]; then
            if [ ! -f "$file" ] || [ -L "$file" ] || ! grep -Fqx '# Removent command line' "$file"; then
                fail "Existing shell configuration preserved: $file"
            fi
        fi
    done
    block="# Removent command line
[ ! -f $(quote "$path_file") ] || . $(quote "$path_file")"
    zsh_dir=${ZDOTDIR:-$HOME}
    case "$zsh_dir" in /*) ;; *) fail 'ZDOTDIR must be absolute' ;; esac
    add_profile "$zsh_dir/.zprofile" "$block"
    add_profile "$zsh_dir/.zshenv" "$block"
    bash_login="$HOME/.profile"
    for file in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        if [ -e "$file" ]; then bash_login=$file; break; fi
    done
    add_profile "$bash_login" "$block"
    add_profile "$HOME/.bashrc" "$block"
    fish_dir="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d"
    case "$fish_dir" in /*) ;; *) fail 'XDG_CONFIG_HOME must be absolute' ;; esac
    # fish single quotes escape backslashes and quotes differently from POSIX sh.
    fish_quote() { printf "'%s'" "$(printf '%s' "$1" | sed "s/\\\\/\\\\\\\\/g; s/'/\\\\'/g")"; }
    add_profile "$fish_dir/removent.fish" "# Removent command line
test ! -f $(fish_quote "$fish_path_file"); or source $(fish_quote "$fish_path_file")"
    for ((i=0; i<profile_count; i++)); do mkdir -p "$(dirname "${profile_files[$i]}")"; done

    temporary=$(mktemp "$config/.path.XXXXXXXX")
    cat > "$temporary" <<EOF
# Removent command line
case ":\$PATH:" in
    *:$quoted_bin:*) ;;
    *) export PATH=$quoted_bin:"\$PATH" ;;
esac
EOF
    mv -f "$temporary" "$path_file"
    temporary=$(mktemp "$config/.path.XXXXXXXX")
    cat > "$temporary" <<EOF
# Removent command line
if not contains -- $(fish_quote "$bin") \$PATH
    set -gx PATH $(fish_quote "$bin") \$PATH
end
EOF
    mv -f "$temporary" "$fish_path_file"
    temporary=
    for ((i=0; i<profile_count; i++)); do
        printf '\n%s\n' "${profile_blocks[$i]}" >> "${profile_files[$i]}"
    done
fi

for name in removent removent-cli; do
    path="$bin/$name"
    if [ -L "$path" ] && [ "$(readlink "$path")" = "$cli" ]; then continue; fi
    # Recheck after profile setup; an unrelated command may have appeared.
    if [ -e "$path" ] || [ -L "$path" ]; then
        owned_link "$path" || fail "Existing command preserved: $path"
    fi
    temporary="$lock/$name"
    ln -s "$cli" "$temporary"
    mv -f "$temporary" "$path"
    temporary=
done
printf 'CLI ready: %s/removent and %s/removent-cli. Open a new terminal if needed.\n' "$bin" "$bin"
