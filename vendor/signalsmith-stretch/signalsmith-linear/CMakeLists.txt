cmake_minimum_required(VERSION 3.24)

add_library(signalsmith-linear INTERFACE)
set_target_properties(signalsmith-linear PROPERTIES INTERFACE_INCLUDE_DIRECTORIES ${CMAKE_CURRENT_SOURCE_DIR}/include)

# MSVC needs an extra flag to handle this many templates
if (MSVC)
	add_compile_options(/bigobj)
	#set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} /bigobj")
endif()

#find_package(BLAS)
#if(BLAS_FOUND)
#	message(STATUS "found BLAS (${BLAS_LIBRARIES})")
#	target_link_options(fft2 INTERFACE ${BLAS_LINKER_FLAGS})
#	target_link_libraries(fft2 INTERFACE BLAS)
#	target_compile_definitions(fft2 INTERFACE SIGNALSMITH_USE_CBLAS)
#else()
#	message(FATAL "no BLAS found")
##	message(STATUS "using OpenBLAS")
##	include(FetchContent)
##	FetchContent_Declare(
##		openblas
##		GIT_REPOSITORY https://github.com/OpenMathLib/OpenBLAS.git
##		GIT_TAG 5ef8b1964658f9cb6a6324a06f6a1a022609b0c5 # 0.3.28
##		GIT_SHALLOW ON
##	)
##	FetchContent_MakeAvailable(openblas)
##	target_link_libraries(fft2 INTERFACE openblas)
##	target_compile_definitions(fft2 INTERFACE SIGNALSMITH_USE_CBLAS)
#endif()

option(SIGNALSMITH_USE_AVAILABLE "use whatever useful dependencies (e.g. IPP) can be found" ON)
option(SIGNALSMITH_USE_ACCELERATE "always use the Accelerate framework for Apple targets (OSX/iOS)" ON)
option(SIGNALSMITH_USE_IPP "always use IPP (Intel Performance Primitives)" OFF)

if(SIGNALSMITH_USE_AVAILABLE)
	find_package(IPP QUIET)
	if(IPP_FOUND)
		set(SIGNALSMITH_USE_IPP BOOLEAN ON)
	endif()
endif()

if(APPLE AND SIGNALSMITH_USE_ACCELERATE)
	message(STATUS "using Accelerate")
	target_link_libraries(signalsmith-linear INTERFACE "-framework Accelerate" "cblas")
	target_compile_definitions(signalsmith-linear INTERFACE SIGNALSMITH_USE_ACCELERATE ACCELERATE_NEW_LAPACK)
elseif(SIGNALSMITH_USE_IPP)
	find_package(IPP REQUIRED)
	message(STATUS "using IPP")
	target_link_libraries(signalsmith-linear INTERFACE IPP::ippcore IPP::ipps IPP::ippi)
	target_compile_definitions(signalsmith-linear INTERFACE SIGNALSMITH_USE_IPP)
endif()