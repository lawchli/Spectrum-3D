pluginManagement {
    repositories { google(); mavenCentral(); gradlePluginPortal() }
    // Both Android plugin IDs live in the same cached AGP artifact; no separate marker is needed.
    resolutionStrategy.eachPlugin {
        if (requested.id.id == "com.android.library") useModule("com.android.tools.build:gradle:${requested.version ?: "8.13.0"}")
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories { google(); mavenCentral() }
}
rootProject.name = "Spectrum3D"
include(":app", ":spectrum")
