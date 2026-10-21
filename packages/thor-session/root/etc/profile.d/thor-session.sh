# The session on the first console (its autologin: getty@tty1's thor-autologin.conf): sway through uwsm
# (thor-session); once it has run and ended, logged out, so that the autologin starts a new one. When it
# can't go on (it keeps failing, or ClamShell does), the console stays, saying why.
if [ "$(tty)" = /dev/tty1 ] && uwsm check may-start > /dev/null 2>&1; then
    /usr/bin/thor-session && exit
fi
