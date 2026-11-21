#!/usr/bin/env bash
# A stand-in for WLCS 1.8.1's runner in yuna-bench's tests (crates/yuna-bench/src/wlcs/mod.rs).
#
#   fake-wlcs.sh <scenario> [--gtest_list_tests] [--gtest_filter=<filter>] [...]
#
# Its first argument, in place of the integration module, is a scenario: lines `<test>
# <behaviour>`, in the order it runs them. It lists them as `--gtest_list_tests` does, or runs
# those `--gtest_filter` selects, printing what WLCS's printer and listener print, each test as
# its behaviour says:
#
#   ok, fail, skip; unsupported (the module's marker on standard error, then OK); timeout (one of
#   WLCS's own waits given up); crash (SIGSEGV); crash-after (OK, then SIGSEGV); hang (silent until
#   killed, with a child of its own in its process group); hang-alone (the same, the runner's own
#   process silent, its pid written to `pid` beside the scenario); spam (prints without end);
#   pass-first (passes the first time it runs, fails after).
#
# Each call appends its arguments to `invocations`, beside the scenario; pass-first counts its
# test's runs there too. It leaves no core file when it crashes.
set -u -f
ulimit -c 0
scenario=$1
shift
folder=$(dirname "$scenario")
printf '%s\n' "$*" >>"$folder/invocations"

list=false
filter='*'
for argument in "$@"; do
    case $argument in
    --gtest_list_tests) list=true ;;
    --gtest_filter=*) filter=${argument#--gtest_filter=} ;;
    esac
done

tests=()
behaviours=()
while read -r name behaviour; do
    case $name in '' | '#'*) continue ;; esac
    tests+=("$name")
    behaviours+=("$behaviour")
done <"$scenario"

if $list; then
    suite=
    for name in "${tests[@]}"; do
        if [ "${name%%.*}" != "$suite" ]; then
            suite=${name%%.*}
            echo "$suite."
        fi
        case ${name#*.} in
        */*) echo "  ${name#*.}  # GetParam() = 0" ;;
        *) echo "  ${name#*.}" ;;
        esac
    done
    exit 0
fi

# gtest's filter: positive patterns, then after a '-' negative ones, each separated by ':'. With
# globbing off (set -f), an unquoted pattern on the right of == matches as gtest's does.
positive=${filter%%-*}
negative=
case $filter in *-*) negative=${filter#*-} ;; esac
matches() {
    local IFS=: pattern
    for pattern in $2; do
        [[ $1 == $pattern ]] && return 0
    done
    return 1
}

echo "Note: Google Test filter = $filter"
echo "[==========] Running the scenario's tests."
failed=()
ran=0
for index in "${!tests[@]}"; do
    name=${tests[$index]}
    case $name in DISABLED_* | *.DISABLED_* | */DISABLED_*) continue ;; esac
    matches "$name" "$positive" || continue
    if [ -n "$negative" ] && matches "$name" "$negative"; then continue; fi
    ran=$((ran + 1))
    echo "[ RUN      ] $name"
    case ${behaviours[$index]} in
    ok) echo "[       OK ] $name (1 ms)" ;;
    fail)
        echo "/build/wlcs/tests/fake.cpp:12: Failure"
        echo "Value of: surface.configured"
        echo "[  FAILED  ] $name (2 ms)"
        failed+=("$name")
        ;;
    timeout)
        echo "unknown file: Failure"
        echo 'C++ exception with description "Timeout waiting for condition" thrown in the test body.'
        echo "[  FAILED  ] $name (10002 ms)"
        failed+=("$name")
        ;;
    skip)
        echo "[          ] Missing extension: wp_fake_v1>= 1"
        echo "[     SKIP ] $name (1ms)"
        ;;
    unsupported)
        echo "yuna-wlcs: unsupported: touch" >&2
        echo "[       OK ] $name (1 ms)"
        ;;
    crash) kill -SEGV $$ ;;
    crash-after)
        echo "[       OK ] $name (1 ms)"
        kill -SEGV $$
        ;;
    hang) sleep 600 ;;
    hang-alone)
        echo $$ >"$folder/pid"
        exec sleep 600
        ;;
    spam) while :; do echo "spam"; done ;;
    pass-first)
        count="$folder/runs-${name//\//_}"
        runs=0
        if [ -f "$count" ]; then runs=$(<"$count"); fi
        echo $((runs + 1)) >"$count"
        if [ "$runs" -eq 0 ]; then
            echo "[       OK ] $name (1 ms)"
        else
            echo "[  FAILED  ] $name (1 ms)"
            failed+=("$name")
        fi
        ;;
    *)
        echo "fake-wlcs.sh: no behaviour ${behaviours[$index]}" >&2
        exit 3
        ;;
    esac
done
echo "[==========] $ran tests from 1 test cases run. (5ms total elapsed)"
echo "[  PASSED  ] $((ran - ${#failed[@]})) tests"
if [ ${#failed[@]} -gt 0 ]; then
    echo "[  FAILED  ] ${#failed[@]} tests failed:"
    for name in "${failed[@]}"; do echo "[  FAILED  ] $name"; done
    exit 1
fi
exit 0
