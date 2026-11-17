{
  perSystem =
    { builds, ... }:
    {
      packages.bootvine-static = builds.bootvine-static;
    };
}
