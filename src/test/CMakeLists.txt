cmake_minimum_required(VERSION 3.6)
project(dualie_tests C)
set(CMAKE_C_STANDARD 11)

add_executable(dualie_tests
    run_tests.c
    test_caps_layer.c
    test_serialize.c
)

target_include_directories(dualie_tests PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}
)
