# syntax=docker/dockerfile:1
#
# Build the SvelteKit frontend with the adapter-node output.
#
# Build context must be the repository root (the Dockerfile copies from `web/`):
#   docker build -f deploy/docker/web.Dockerfile --build-arg VITE_API_BASE=http://localhost:3080 .

FROM docker.io/oven/bun:1 AS build
WORKDIR /app

COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile

COPY web/ ./
ARG VITE_API_BASE=http://localhost:3080
ENV VITE_API_BASE=${VITE_API_BASE}
RUN bun run build

# Runtime dependencies only. adapter-node externalizes packages, so they must
# be present at runtime rather than bundled into build/.
FROM docker.io/oven/bun:1 AS deps
WORKDIR /app
COPY web/package.json web/bun.lock ./
# --ignore-scripts skips the root `prepare` (svelte-kit sync), whose binary is a
# devDependency and absent here; no runtime dependency needs an install script.
RUN bun install --frozen-lockfile --production --ignore-scripts

FROM docker.io/library/node:22-bookworm-slim AS runtime
WORKDIR /app
ENV NODE_ENV=production
ENV PORT=3000

COPY --from=build /app/build ./build
COPY --from=deps /app/node_modules ./node_modules
COPY --from=build /app/package.json ./package.json

EXPOSE 3000
CMD ["node", "build"]
