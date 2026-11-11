for d in /sdcard /data/media/0; do if [ -f $d/abl-flash.sh ]; then (id; sh $d/abl-flash.sh) > $d/thor-abl-flash.log 2>&1; break; fi; done
