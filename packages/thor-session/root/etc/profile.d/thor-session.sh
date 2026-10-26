# The session on the first console (its autologin: getty@tty1's thor-autologin.conf): ClamShell's
# compositor (thor-session); once it has run and ended, logged out, so that the autologin starts a new
# one. When it can't go on (it keeps failing, or ClamShell does), the console stays, saying why. A
# bring-up image (/etc/thor/no-session) stays at the shell; thor-session starts the session by hand.
if [ "$(tty)" = /dev/tty1 ] && [ -e /etc/thor/no-session ]; then
    echo "The session is off at boot (/etc/thor/no-session); thor-session starts it."
elif [ "$(tty)" = /dev/tty1 ] && ! systemctl --user -q is-active clamshell.service; then
    /usr/bin/thor-session && exit
fi
