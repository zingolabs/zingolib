#include "AppPersonalities.h"

#include <stdexcept>

bool app_personalities_cxx_catches(void) {
    try {
        throw std::runtime_error("the C++ personality routine");
    } catch (const std::runtime_error &) {
        return true;
    }
}
