cmake_minimum_required(VERSION 3.20)

project(latentdeck_spout_bridge LANGUAGES CXX)

if(NOT WIN32 OR NOT MSVC)
    message(FATAL_ERROR "LatentDeck Spout2 bridge requires Windows MSVC")
endif()

if(NOT DEFINED LATENTDECK_SPOUT2_SOURCE_DIR)
    message(FATAL_ERROR "LATENTDECK_SPOUT2_SOURCE_DIR is required")
endif()

set(SPOUT_BUILD_ARM OFF CACHE BOOL "" FORCE)
set(SPOUT_BUILD_CMT OFF CACHE BOOL "" FORCE)
set(SPOUT_BUILD_LIBRARY OFF CACHE BOOL "" FORCE)
set(SPOUT_BUILD_SPOUTDX ON CACHE BOOL "" FORCE)
set(SPOUT_NO_GLOBAL_WARNING ON CACHE BOOL "" FORCE)
set(SKIP_INSTALL_ALL ON CACHE BOOL "" FORCE)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>DLL")

add_subdirectory("${LATENTDECK_SPOUT2_SOURCE_DIR}" spout2 EXCLUDE_FROM_ALL)

add_library(latentdeck_spout_bridge STATIC spout_bridge.cpp spout_bridge.h)
target_compile_features(latentdeck_spout_bridge PRIVATE cxx_std_17)
target_compile_definitions(latentdeck_spout_bridge PRIVATE
    WIN32_LEAN_AND_MEAN
    NOMINMAX
    SPOUT_BUILD_STATIC
)
target_compile_options(latentdeck_spout_bridge PRIVATE /W4 /WX)
target_link_libraries(latentdeck_spout_bridge PRIVATE SpoutDX12_static)

set(_latentdeck_archive_dir "${CMAKE_BINARY_DIR}/lib")
foreach(_target latentdeck_spout_bridge SpoutDX12_static SpoutDX_static Spout_static)
    set_target_properties(${_target} PROPERTIES
        ARCHIVE_OUTPUT_DIRECTORY "${_latentdeck_archive_dir}"
        ARCHIVE_OUTPUT_DIRECTORY_DEBUG "${_latentdeck_archive_dir}"
        ARCHIVE_OUTPUT_DIRECTORY_RELEASE "${_latentdeck_archive_dir}"
        ARCHIVE_OUTPUT_DIRECTORY_RELWITHDEBINFO "${_latentdeck_archive_dir}"
        ARCHIVE_OUTPUT_DIRECTORY_MINSIZEREL "${_latentdeck_archive_dir}"
    )
endforeach()
