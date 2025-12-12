#!/bin/bash

set -eoux

ulimit -s 16384
calc-witness /setup/verify_for_guest_graph.bin /mnt/input.json output.wtns
rapidsnark /setup/verify_for_guest_final.zkey output.wtns /mnt/proof.json /mnt/public.json