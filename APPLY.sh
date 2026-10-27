#!/usr/bin/env bash
# BATCH 02 apply helper.
#
# Unzip batch-02-recorded-datasets.zip over the repository root, then run this
# script to stage the changes. It does NOTHING except `git add -A`: it never
# commits, tags, pushes, rebases or rewrites history. Reviewing and committing
# the staged files is the integrator's job.
set -euo pipefail
git add -A
