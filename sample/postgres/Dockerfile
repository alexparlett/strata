# The sample project's PostgreSQL half — a throwaway fixture, not a deployment.
#
# The seed is **baked in** rather than bind-mounted, because a mount has to survive whichever VM
# is underneath (colima, Docker Desktop, Rancher) and this way `docker run` alone is enough.
# `/docker-entrypoint-initdb.d` runs once, on an empty data directory: drop the container and the
# next one re-seeds itself, which is what makes this fixture idempotent.
FROM postgres:17-alpine

COPY init/ /docker-entrypoint-initdb.d/
