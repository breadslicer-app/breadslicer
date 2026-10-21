# Official sample fixtures

Data taken from files real producers wrote, so the writers in this
crate are measured against ground truth instead of home-grown
approximations of each format.

One subdirectory per provider.

## `elegoo-goo-v3/`

`layer-32-runs.txt` holds the pixel runs of a single layer, layer 32
of 1000, from `ROCK.goo`: a sample file Elegoo distributes with the
Mars 5 printer, sliced by Voxeldance Tango 4.0.17.10 at 4098x2560.
The runs are decoded from the vendor's chunk stream, not copied from
it, and the file records the byte count the vendor's own encoding of
those runs occupies.

Elegoo grants no redistribution license for its sample files. What is
reproduced here is 0.1% of one file, a single 2D cross-section that
cannot reconstruct the model, kept for the sole purpose of testing an
independent implementation of a format Elegoo publishes a
specification for.

It pins the chunk encoder's choice between the delta form and the
black, grey and white forms against a real producer's choice for the
same runs. The layer covers every branch of that choice: 2521 runs of
1 pixel, 6 in the 2 to 15 band where both forms cost the same, 81 in
the 16 to 255 band, 1633 needing a length byte, 2 needing 3 of them,
and 863 black against 860 white runs across 212 distinct values.
