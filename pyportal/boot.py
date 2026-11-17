# Tach boot.py: enable a second USB serial channel for Tach data.
# The console channel stays available for the REPL.
# This file runs only after a hard reset (press RESET, or unplug the cable).
import usb_cdc

usb_cdc.enable(console=True, data=True)
