#!/usr/bin/env bash
# The two models the board's hand gestures run (the `hands` feature):
# MediaPipe's palm detector and hand landmark model, as OpenCV's model zoo
# converted them to ONNX (Apache 2.0). Downloaded once into the data
# directory, checked against the sums they were tested with; the board
# never downloads anything itself.
set -euo pipefail

dest="${XDG_DATA_HOME:-$HOME/.local/share}/sinopia/models"
zoo=https://huggingface.co/opencv

models=(
  "palm_detection_mediapipe/palm_detection_mediapipe_2023feb.onnx 78ff51c38496b7fc8b8ebdb6cc8c1abb02fa6c38427c6848254cdaba57fcce7c"
  "handpose_estimation_mediapipe/handpose_estimation_mediapipe_2023feb.onnx db0898ae717b76b075d9bf563af315b29562e11f8df5027a1ef07b02bef6d81c"
)

mkdir -p "$dest"
chmod 0700 "$(dirname "$dest")" "$dest"
for entry in "${models[@]}"; do
  read -r path sum <<< "$entry"
  file="$dest/$(basename "$path")"
  if [ -f "$file" ] && echo "$sum  $file" | sha256sum --check --status; then
    echo "have $(basename "$file")"
    continue
  fi
  tmp="$(mktemp "$dest/.fetch.XXXXXX")"
  trap 'rm -f "$tmp"' EXIT
  curl -fsSL "$zoo/${path%%/*}/resolve/main/${path#*/}" -o "$tmp"
  if ! echo "$sum  $tmp" | sha256sum --check --status; then
    echo "$(basename "$file"): the download does not match its sum" >&2
    exit 1
  fi
  chmod 0600 "$tmp"
  mv "$tmp" "$file"
  trap - EXIT
  echo "fetched $(basename "$file")"
done
echo "models in $dest"
