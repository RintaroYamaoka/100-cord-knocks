#!/usr/bin/env bash
# 実行イメージ (7 言語のツールチェーン) をビルドし、必要なら Vercel Container Registry へ push する。
#
#   bash scripts/build-runner-image.sh                # ローカルに knocks-runtime:local を作るだけ
#   bash scripts/build-runner-image.sh --push <tag>   # VCR へ push (本番が使うのはこちら)
#
# ★このスクリプトは**個人アカウント (rintaroyamaoka) 専用**。
#   会社 (propagate-webcreation) のトークンが環境にあると誤爆するので、その場合は止める。
set -euo pipefail

EXPECTED_USER="rintaroyamaoka-3890"
PROJECT="100-cord-knocks"
REPO="knocks-runtime"
CONTEXT="docker/runner"
LOCAL_TAG="knocks-runtime:local"

push=false
tag="$(date +%Y-%m-%d)"
while [ $# -gt 0 ]; do
  case "$1" in
    --push) push=true ;;
    *) tag="$1" ;;
  esac
  shift
done

# --- アカウントのガード ---------------------------------------------------
if [ -n "${VERCEL_TOKEN:-}" ]; then
  echo "✗ VERCEL_TOKEN が環境にある (会社スコープの可能性)。このスクリプトは個人アカウント専用。" >&2
  echo "  素の環境で実行してください (~/.config/propagate/vercel.env を読み込まない)。" >&2
  exit 1
fi
who="$(vercel whoami 2>/dev/null | tail -1 | tr -d '[:space:]')"
if [ "$who" != "$EXPECTED_USER" ]; then
  echo "✗ vercel のログインが個人アカウントではない (whoami=$who, 期待=$EXPECTED_USER)" >&2
  exit 1
fi

# --- ローカルビルド -------------------------------------------------------
echo "▸ ローカルビルド ($LOCAL_TAG)"
docker build -f "$CONTEXT/Dockerfile" -t "$LOCAL_TAG" "$CONTEXT"
docker run --rm "$LOCAL_TAG" cat /opt/knocks/VERSIONS

if [ "$push" != true ]; then
  echo "▸ push していません (--push を付けると VCR へ上げます)"
  exit 0
fi

# --- VCR へ push ----------------------------------------------------------
# vercel vcr はリンク済みプロジェクトにリポジトリを作る。リンクが無いと
# ディレクトリ名から別プロジェクトを新規作成してしまうので必ず先にリンクする。
echo "▸ プロジェクトをリンク ($PROJECT)"
vercel link --yes --project "$PROJECT" >/dev/null
echo "▸ VCR にログイン"
vercel vcr login docker
echo "▸ push: $REPO:$tag"
vercel vcr build docker "$CONTEXT" "$REPO:$tag" --push
echo "▸ 完了。イメージ参照を確認:"
vercel vcr tag inspect "$REPO" "$tag"
