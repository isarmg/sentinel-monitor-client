#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
installer=$script_dir/../install.sh
grep -qx 'KillMode=mixed' "$script_dir/../xcoc.service"
grep -qx 'TimeoutStopSec=25s' "$script_dir/../xcoc.service"
XCOC_INSTALL_SOURCE_ONLY=1
export XCOC_INSTALL_SOURCE_ONLY
# shellcheck disable=SC1090 # The tested installer path is computed from this script.
. "$installer"

test_root=$(mktemp -d)
trap 'rm -rf -- "$test_root"' 0
mkdir -p "$test_root/fake-bin"
cat > "$test_root/fake-bin/systemctl" <<'STUB'
#!/bin/sh
set -eu
command=$1
printf '%s\n' "$command" >> "$TEST_STATE_DIR/calls"
if [ "$command" = daemon-reload ]; then : > "$TEST_STATE_DIR/reload-seen"; fi
if [ "$command" = "$TEST_FAIL_ON" ] && [ ! -e "$TEST_STATE_DIR/failed-once" ]; then
  if [ "$command" != is-active ] || [ -e "$TEST_STATE_DIR/reload-seen" ]; then
    : > "$TEST_STATE_DIR/failed-once"
    exit 1
  fi
fi
case "$command" in
  is-active) [ "$(cat "$TEST_STATE_DIR/active")" = yes ] ;;
  is-enabled) [ "$(cat "$TEST_STATE_DIR/enabled")" = yes ] ;;
  stop) printf 'no\n' > "$TEST_STATE_DIR/active" ;;
  start) printf 'yes\n' > "$TEST_STATE_DIR/active" ;;
  enable) printf 'yes\n' > "$TEST_STATE_DIR/enabled" ;;
  disable) printf 'no\n' > "$TEST_STATE_DIR/enabled" ;;
  daemon-reload) : ;;
  *) echo "Unexpected systemctl command: $command" >&2; exit 1 ;;
esac
STUB
cat > "$test_root/fake-bin/install" <<'STUB'
#!/bin/sh
set -eu
if [ "$TEST_FAIL_ON" = install-binary ] && [ "${1:-}" = -m ] && [ "${2:-}" = 0755 ] && [ ! -e "$TEST_STATE_DIR/failed-once" ]; then
  : > "$TEST_STATE_DIR/failed-once"
  exit 1
fi
if [ "$TEST_FAIL_ON" = install-unit ] && [ "${1:-}" = -m ] && [ "${2:-}" = 0644 ] && [ ! -e "$TEST_STATE_DIR/failed-once" ]; then
  : > "$TEST_STATE_DIR/failed-once"
  exit 1
fi
exec /usr/bin/install "$@"
STUB
chmod 755 "$test_root/fake-bin/systemctl" "$test_root/fake-bin/install"
PATH="$test_root/fake-bin:$PATH"
export PATH

fail() { echo "FAIL: $*" >&2; exit 1; }
assert_file_value() {
  actual=$(cat "$1")
  [ "$actual" = "$2" ] || fail "$1: expected '$2', got '$actual'"
}

run_case() {
  name=$1
  old_install=$2
  old_active=$3
  old_enabled=$4
  desired_boot=$5
  fail_on=$6
  expected_success=$7
  case_root=$test_root/$name
  TEST_STATE_DIR=$case_root/state
  TEST_FAIL_ON=$fail_on
  export TEST_STATE_DIR TEST_FAIL_ON
  mkdir -p "$case_root/source" "$case_root/target/bin" "$case_root/target/systemd" "$TEST_STATE_DIR"
  source_binary=$case_root/source/xcoc
  source_unit=$case_root/source/xcoc.service
  target_binary=$case_root/target/bin/xcoc
  target_unit=$case_root/target/systemd/xcoc.service
  cat > "$source_binary" <<'STUB'
#!/bin/sh
set -eu
[ "$*" = 'media-worker --check' ] || exit 2
[ "$TEST_FAIL_ON" != media-check ]
STUB
  printf 'new-unit\n' > "$source_unit"
  chmod 755 "$source_binary"
  printf '%s\n' "$old_active" > "$TEST_STATE_DIR/active"
  printf '%s\n' "$old_enabled" > "$TEST_STATE_DIR/enabled"
  if [ "$old_install" = yes ]; then
    printf 'old-binary\n' > "$target_binary"
    printf 'old-unit\n' > "$target_unit"
    chmod 755 "$target_binary"
  fi

  if install_client "$source_binary" "$source_unit" "$target_binary" "$target_unit" "$desired_boot" > "$case_root/output" 2>&1; then
    [ "$expected_success" = yes ] || fail "$name unexpectedly succeeded"
  else
    [ "$expected_success" = no ] || fail "$name unexpectedly failed: $(cat "$case_root/output")"
  fi

  if [ "$expected_success" = yes ]; then
    cmp "$target_binary" "$source_binary" || fail "$name installed wrong binary"
    assert_file_value "$target_unit" new-unit
    assert_file_value "$TEST_STATE_DIR/active" yes
    assert_file_value "$TEST_STATE_DIR/enabled" "$desired_boot"
  else
    if [ "$old_install" = yes ]; then
      assert_file_value "$target_binary" old-binary
      assert_file_value "$target_unit" old-unit
    else
      [ ! -e "$target_binary" ] || fail "$name left a binary after failed fresh install"
      [ ! -e "$target_unit" ] || fail "$name left a unit after failed fresh install"
    fi
    assert_file_value "$TEST_STATE_DIR/active" "$old_active"
    assert_file_value "$TEST_STATE_DIR/enabled" "$old_enabled"
  fi
  [ "$(find "$case_root/target/bin" -mindepth 1 -maxdepth 1 | wc -l)" -eq "$(if [ -e "$target_binary" ]; then printf 1; else printf 0; fi)" ] || fail "$name left a temporary or rollback binary"
  [ "$(find "$case_root/target/systemd" -mindepth 1 -maxdepth 1 | wc -l)" -eq "$(if [ -e "$target_unit" ]; then printf 1; else printf 0; fi)" ] || fail "$name left a temporary unit"
  printf 'PASS %s\n' "$name"
}

run_case upgrade_success yes yes yes no none yes
run_case media_preflight_failure yes yes yes no media-check no
run_case stop_failure yes yes yes no stop no
run_case binary_copy_failure yes yes yes no install-binary no
run_case unit_copy_failure yes yes yes no install-unit no
run_case reload_failure yes yes yes no daemon-reload no
run_case enable_failure yes yes yes no disable no
run_case start_failure yes yes yes no start no
run_case health_failure yes yes yes no is-active no
run_case inactive_restore yes no no yes daemon-reload no
run_case fresh_failure no no no yes start no
run_case fresh_success no no no yes none yes
