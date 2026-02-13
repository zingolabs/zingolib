# Simple wrapper for scripts with printed status messages.
#
# Running `make` or `make stagex` will leverage the steps below
# to check compatibility and build the binaries via StageX.

.PHONY: stagex compat build

stagex:	compat build
	@echo "stagex build completed via make."

compat:
	@echo "Beginning Compatibility Check step."
	@./utils/compat.sh
	@echo "  [PASS]  Compatibility Check passed."

build:
	@echo "Entering Build step."
	@./utils/build.sh
	@echo "Build step complete."
