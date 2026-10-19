#!/bin/bash
# What a session runs: the system's and the user's units with their processes, and per cgroup its
# memory now and its CPU time over a window (default 30 s). Read-only.
WINDOW=${1:-30}
CG=/sys/fs/cgroup

echo "== system services running"
systemctl list-units --type=service --state=running --no-legend --plain | awk '{print "  " $1}'

echo "== user units running (services, scopes)"
systemctl --user list-units --type=service,scope --state=running --no-legend --plain | awk '{print "  " $1}'

echo "== processes outside units of their own (in the login session's scope)"
for p in $(cat $CG/user.slice/user-$(id -u).slice/session-*.scope/cgroup.procs 2>/dev/null); do
    echo "  $p $(tr '\0' ' ' < /proc/$p/cmdline 2>/dev/null | cut -c1-100)"
done

# Leaf-ish cgroups worth reporting: system services, user services and scopes, and the login session.
groups() {
    find $CG/system.slice -mindepth 1 -maxdepth 1 -type d
    find $CG/user.slice/user-$(id -u).slice -maxdepth 1 -name 'session-*.scope' -type d
    find $CG/user.slice/user-$(id -u).slice/user@$(id -u).service -mindepth 2 -maxdepth 3 -type d \( -name '*.service' -o -name '*.scope' \)
    find $CG -maxdepth 2 -path '*lxc*' -type d
}
usage() { awk '/^usage_usec/ {print $2}' "$1/cpu.stat" 2>/dev/null; }

declare -A before
for g in $(groups); do before[$g]=$(usage "$g"); done
sleep "$WINDOW"
echo "== per cgroup: memory now (MiB), CPU over ${WINDOW} s (ms), processes"
for g in $(groups); do
    mem=$(( $(cat "$g/memory.current" 2>/dev/null || echo 0) / 1048576 ))
    after=$(usage "$g")
    cpu=$(( (${after:-0} - ${before[$g]:-0}) / 1000 ))
    procs=$(cat "$g/cgroup.procs" 2>/dev/null | wc -l)
    [ "$mem" -eq 0 ] && [ "$cpu" -eq 0 ] && continue
    printf "  %6d %7d %4d  %s\n" "$mem" "$cpu" "$procs" "${g#$CG/}"
done | sort -rn

echo "== whole machine"
free -m | sed 's/^/  /'
echo "  load: $(cat /proc/loadavg)"
