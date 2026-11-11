for d in /sdcard /data/media/0; do if [ -f $d/android-backup.sh ]; then (id; sh $d/android-backup.sh) > $d/thor-backup.log 2>&1; break; fi; done
