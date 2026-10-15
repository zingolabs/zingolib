# Simple wrapper for scripts with printed status messages.
#
# Running `make` or `make stagex` will leverage the steps below
# to check compatibility and build the binaries via StageX.

.PHONY: stagex compat build load create interact

stagex:	compat build
	@echo "[Stageˣ] build completed via make."

compat:
	@echo "Beginning Compatibility Check step."
	@./utils/compat.sh
	@echo "  [PASS]  Compatibility Check passed."

build:
	@echo "Entering Build step."
	@./utils/build.sh
	@echo "Build step complete."

load:
	@echo "Attempting to load OCI image into local docker image store."
	@./utils/load_image.sh
	@echo "make load step complete."
