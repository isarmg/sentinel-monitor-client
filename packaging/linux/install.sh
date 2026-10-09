#!/bin/sh
set -eu

install_client() (
  set -eu
  require() { "$@" || exit $?; }
  source_binary=$1
  source_unit=$2
  target_binary=$3
  target_unit=$4
  boot_start=$5

  if [ ! -x "$source_binary" ]; then
    echo "Build or extract the release binary first: $source_binary" >&2
    exit 1
  fi
  if [ ! -f "$source_unit" ]; then
    echo "Missing service unit: $source_unit" >&2
    exit 1
  fi
  if [ -L "$target_binary" ] || [ -L "$target_unit" ]; then
    echo 'Refusing to replace a symbolic link at an installation path.' >&2
    exit 1
  fi
  require install -d -m 0755 "$(dirname -- "$target_binary")" "$(dirname -- "$target_unit")"

  had_binary=no
  had_unit=no
  was_active=no
  was_enabled=no
  [ ! -f "$target_binary" ] || had_binary=yes
  if [ -f "$target_unit" ]; then
    had_unit=yes
    if systemctl is-active --quiet xcoc.service; then was_active=yes; fi
    if systemctl is-enabled --quiet xcoc.service; then was_enabled=yes; fi
  fi

  # Keep the old binary on the same filesystem so rollback can restore it by rename.
  backup_dir=$(mktemp -d "${target_binary}.rollback.XXXXXX") || exit $?
  mutation_started=no
  completed=no
  temporary_binary=
  temporary_unit=
  # shellcheck disable=SC2329 # Invoked by the exit trap.
  rollback() {
    exit_status=$?
    trap - 0 HUP INT TERM
    set +e
    if [ -n "$temporary_binary" ]; then rm -f -- "$temporary_binary"; fi
    if [ -n "$temporary_unit" ]; then rm -f -- "$temporary_unit"; fi
    if [ "$completed" != yes ] && [ "$mutation_started" = yes ]; then
      echo 'xcoc installation failed; restoring the previous installation.' >&2
      rollback_failed=no
      if [ -f "$target_unit" ] && ! systemctl stop xcoc.service >/dev/null 2>&1; then
        echo 'Rollback could not stop the installed xcoc service.' >&2
        rollback_failed=yes
      fi
      systemctl disable xcoc.service >/dev/null 2>&1
      if [ "$had_binary" = yes ]; then
        if ! mv -f -- "$backup_dir/binary" "$target_binary"; then
          echo "Rollback could not restore $target_binary" >&2
          rollback_failed=yes
        fi
      else
        if ! rm -f -- "$target_binary"; then rollback_failed=yes; fi
      fi
      if [ "$had_unit" = yes ]; then
        if ! cp -p -- "$backup_dir/unit" "$target_unit"; then
          echo "Rollback could not restore $target_unit" >&2
          rollback_failed=yes
        fi
      else
        if ! rm -f -- "$target_unit"; then rollback_failed=yes; fi
      fi
      if ! systemctl daemon-reload; then rollback_failed=yes; fi
      if [ "$had_unit" = yes ]; then
        if [ "$was_enabled" = yes ]; then
          if ! systemctl enable xcoc.service; then rollback_failed=yes; fi
        else
          if ! systemctl disable xcoc.service; then rollback_failed=yes; fi
        fi
        if [ "$was_active" = yes ] && ! systemctl start xcoc.service; then
          echo 'Rollback could not restart the previous xcoc service.' >&2
          rollback_failed=yes
        fi
      fi
      if [ "$rollback_failed" = yes ]; then
        echo "Rollback needs manual attention; backups were kept at $backup_dir" >&2
        exit "$exit_status"
      fi
    fi
    rm -rf -- "$backup_dir"
    exit "$exit_status"
  }
  trap 'rollback' 0
  trap 'exit 1' HUP INT TERM

  if [ "$had_binary" = yes ]; then require cp -p -- "$target_binary" "$backup_dir/binary"; fi
  if [ "$had_unit" = yes ]; then require cp -p -- "$target_unit" "$backup_dir/unit"; fi

  mutation_started=yes
  if [ "$had_unit" = yes ]; then require systemctl stop xcoc.service; fi
  temporary_binary="${target_binary}.new.$$"
  temporary_unit="${target_unit}.new.$$"
  require install -m 0755 "$source_binary" "$temporary_binary"
  require mv -f -- "$temporary_binary" "$target_binary"
  require install -m 0644 "$source_unit" "$temporary_unit"
  require mv -f -- "$temporary_unit" "$target_unit"
  require systemctl daemon-reload
  if [ "$boot_start" = yes ]; then
    require systemctl enable xcoc.service
  else
    require systemctl disable xcoc.service
  fi
  require systemctl start xcoc.service
  require systemctl is-active --quiet xcoc.service
  completed=yes
  rm -rf -- "$backup_dir"
  trap - 0 HUP INT TERM
)

main() {
  if [ "$(id -u)" -ne 0 ]; then
    echo 'Run this installer as root.' >&2
    exit 1
  fi
  if [ ! -d /run/systemd/system ] || ! command -v systemctl >/dev/null 2>&1; then
    echo 'A running systemd system manager is required.' >&2
    exit 1
  fi

  script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
  source_binary=$script_dir/../../target/release/xcoc
  source_unit=$script_dir/xcoc.service
  target_binary=/usr/local/bin/xcoc
  target_unit=/etc/systemd/system/xcoc.service

  PATH=/usr/local/bin:/usr/bin:/bin
  export PATH
  for tool in ffmpeg ffprobe; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      echo "$tool must be available in the systemd service PATH." >&2
      exit 1
    fi
  done

  printf 'Start xcoc automatically at boot? [Y/n] '
  if ! IFS= read -r answer; then answer=; fi
  case "$answer" in
    ''|y|Y|yes|YES|Yes) boot_start=yes ;;
    n|N|no|NO|No) boot_start=no ;;
    *) echo 'Enter yes or no.' >&2; exit 1 ;;
  esac

  install_client "$source_binary" "$source_unit" "$target_binary" "$target_unit" "$boot_start"
  echo 'xcoc is running. Pair a camera with xcoc setup.'
}

if [ "${XCOC_INSTALL_SOURCE_ONLY:-0}" != 1 ]; then main "$@"; fi
