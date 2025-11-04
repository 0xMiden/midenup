# Tutorial

A typical usage of midenup and miden might look like the following:

1. midenup has been downloade and correctly configured following the instructions of the [Installation](installation.md) page or [README](https://github.com/0xMiden/midenup)
2. The latest stable toolchain can then be installed:

   ```sh
   midenup install stable
   ```

3. With the toolchain now installed, the installed components can be inspected with the following command:

   ```sh
   miden help toolchain
   ```

4. On this list, components that require initialization will display their corresponding commmand. One such component is the miden client, which can be initialized like so:

   ```sh
   miden client init --network devnet
   ```

   (`devnet` is used as an example).

5. With the client now initialized, an account can be created and deployed using code from a custom miden project. To start, create a new miden project:
   ```sh
   miden new miden_project && cd miden_project
   ```
6. If said project requires a specific toolchain version, for instance 0.17.0, then it can be set with the following command:
   ```sh
   midenup set 0.17.0
   ```
   Note that if the toolchain is not already installed, midenup/miden will automatically install it as soon as it detects that it is required.
7. With the project now generated and the required toolchain established, the `src/lib.rs` can be modified with any desired additions. Afterwards, a build can be issued:
   ```sh
   miden build
   ```
   Once compilation finishes, a message displaying the location of the generated Miden Package will be shown.
8. With the generated Miden Package, an account can be created and deployed with the following command:
   ```sh
   miden client new-account --account-type regular-account-updatable-code -p /path/to/package.masp
   ```
