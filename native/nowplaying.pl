# Loads the notch-utils Now Playing library into Apple's signed perl and runs one function.
# Usage: perl nowplaying.pl <library.dylib> stream|command
use strict;
use warnings;
use DynaLoader;
my ($lib, $mode) = @ARGV;
my $handle = DynaLoader::dl_load_file($lib, 0) or die "load: " . DynaLoader::dl_error() . "\n";
my $name = $mode eq 'stream' ? 'np_stream' : 'np_command';
my $sym = DynaLoader::dl_find_symbol($handle, $name) or die "symbol $name missing\n";
DynaLoader::dl_install_xsub('main::run', $sym);
run();
